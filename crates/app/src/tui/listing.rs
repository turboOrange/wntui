use wntui_core::{
    engine::EngineState,
    filter::Filter,
    model::{Node, NodeKey, OnlineStatus, Role},
};

use super::app::AppState;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortField {
    Address,
    Name,
    Status,
    Role,
    Voltage,
    Firmware,
    Age,
}

impl SortField {
    pub fn next(self) -> Self {
        match self {
            Self::Address => Self::Name,
            Self::Name => Self::Status,
            Self::Status => Self::Role,
            Self::Role => Self::Voltage,
            Self::Voltage => Self::Firmware,
            Self::Firmware => Self::Age,
            Self::Age => Self::Address,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Address => "address",
            Self::Name => "name",
            Self::Status => "status",
            Self::Role => "role",
            Self::Voltage => "voltage",
            Self::Firmware => "firmware",
            Self::Age => "age",
        }
    }

    // Words accepted by `:sort <field> [asc|desc]`.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_lowercase().as_str() {
            "addr" | "address" => Some(Self::Address),
            "name" => Some(Self::Name),
            "status" | "state" => Some(Self::Status),
            "role" => Some(Self::Role),
            "voltage" | "volt" => Some(Self::Voltage),
            "firmware" | "fw" => Some(Self::Firmware),
            "age" => Some(Self::Age),
            _ => None,
        }
    }
}

pub struct NodeView<'a> {
    pub network: Option<u32>,
    pub sort: SortField,
    pub reverse: bool,
    pub filter: Option<&'a Filter>,
    pub now_ms: u64,
}

// Cached filter+sort projection: the ordered node keys plus the inputs it
// was computed from. Recomputes only when the store version, the filter
// text, the sort, or the network selection moved — never per frame.
#[derive(Clone, Debug, Default)]
pub struct Projection {
    keys: Vec<NodeKey>,
    store_version: u64,
    filter_text: String,
    sort: Option<SortField>,
    reverse: bool,
    network: Option<u32>,
    // Nodes in the selected network before the filter ran; cached so the
    // "N of M" line and status counts never walk the whole store.
    total: usize,
    online: usize,
    uncertain: usize,
    offline: usize,
    // Second-resolution bucket of the last refresh; only time-dependent
    // filters (age:) move the cache, and only once per second.
    second: Option<u64>,
    // Bumped every time `keys` is rebuilt, for tests and debugging.
    pub recomputes: u32,
}

// The projection inputs, extracted from AppState so the cache can be
// refreshed without borrowing the state that contains it.
#[derive(Clone, Debug)]
pub struct ProjectionInputs<'a> {
    pub filter: Option<&'a Filter>,
    pub filter_text: &'a str,
    pub sort: SortField,
    pub reverse: bool,
    pub network: Option<u32>,
    pub now_ms: u64,
}

impl<'a> ProjectionInputs<'a> {
    pub fn from_app(app: &'a AppState) -> Self {
        Self {
            filter: app.filter.as_ref(),
            filter_text: &app.filter_text,
            sort: app.sort,
            reverse: app.sort_reverse,
            network: app.selected_network,
            now_ms: app.now_ms,
        }
    }
}

impl Projection {
    pub fn keys(&self) -> &[NodeKey] {
        &self.keys
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    // Nodes in the selected network before the filter ran.
    pub fn total(&self) -> usize {
        self.total
    }

    // Status counts of the projection itself (post-filter).
    pub fn status_counts(&self) -> (usize, usize, usize) {
        (self.online, self.uncertain, self.offline)
    }

    // Rebuilds the projection when any input moved. The filter is matched by
    // its source text, not the parsed tree: two different trees from the
    // same text are impossible (the parser is deterministic) and the text
    // is cheaper to compare.
    pub fn refresh(&mut self, engine: &EngineState, inputs: &ProjectionInputs<'_>) {
        let store_version = engine.store.version();
        let sort = inputs.sort;
        let reverse = inputs.reverse;
        let network = inputs.network;
        // Age predicates are second-resolution, so a once-per-second bucket
        // keeps them fresh without per-frame recompute.
        let second = if inputs.filter.is_some_and(Filter::uses_time) {
            Some(inputs.now_ms / 1000)
        } else {
            None
        };

        if self.sort == Some(sort)
            && self.reverse == reverse
            && self.network == network
            && self.filter_text == inputs.filter_text
            && self.second == second
            && self.store_version == store_version
        {
            return;
        }

        self.recomputes += 1;
        self.store_version = store_version;
        self.filter_text = inputs.filter_text.to_owned();
        self.sort = Some(sort);
        self.reverse = reverse;
        self.network = network;
        self.second = second;

        let view = NodeView {
            network,
            sort,
            reverse,
            filter: inputs.filter,
            now_ms: inputs.now_ms,
        };
        let (nodes, counts) = view.walk(engine);
        self.total = counts.total;
        self.online = counts.online;
        self.uncertain = counts.uncertain;
        self.offline = counts.offline;
        self.keys = nodes.iter().map(|node| node.key).collect();
    }
}

impl<'a> NodeView<'a> {
    pub fn from_app(app: &'a AppState) -> Self {
        Self {
            network: app.selected_network,
            sort: app.sort,
            reverse: app.sort_reverse,
            filter: app.filter.as_ref(),
            now_ms: app.now_ms,
        }
    }

    // Resolves cached keys back to nodes; keys whose nodes vanished since
    // the last refresh are skipped (the next refresh drops them for real).
    pub fn resolve<'e>(&self, engine: &'e EngineState, keys: &[NodeKey]) -> Vec<&'e Node> {
        keys.iter()
            .filter_map(|key| engine.store.node(key))
            .collect()
    }

    // The single production walk: one pass over the store produces the
    // filtered+sorted node list plus the totals the header lines display.
    // The projection caches the result; nothing else walks the store.
    pub fn walk<'e>(&self, engine: &'e EngineState) -> (Vec<&'e Node>, WalkCounts) {
        let mut total = 0;
        let mut online = 0;
        let mut uncertain = 0;
        let mut offline = 0;
        let mut nodes: Vec<&Node> = Vec::new();

        for node in engine.store.nodes() {
            if !self.network.is_none_or(|id| node.key.network_id == id) {
                continue;
            }
            total += 1;
            match node.online {
                OnlineStatus::Online => online += 1,
                OnlineStatus::Uncertain => uncertain += 1,
                OnlineStatus::Offline => offline += 1,
            }
            if self
                .filter
                .is_none_or(|filter| filter.matches(node, self.now_ms))
            {
                nodes.push(node);
            }
        }

        nodes.sort_by(|left, right| self.compare(left, right));

        if self.reverse {
            nodes.reverse();
        }

        (
            nodes,
            WalkCounts {
                total,
                online,
                uncertain,
                offline,
            },
        )
    }

    fn compare(&self, left: &Node, right: &Node) -> std::cmp::Ordering {
        match self.sort {
            SortField::Address => left.key.cmp(&right.key),
            SortField::Name => name_of(left).cmp(name_of(right)),
            // Offline < uncertain < online; the enum order is already that.
            SortField::Status => left.online.cmp(&right.online),
            SortField::Role => role_rank(left).cmp(&role_rank(right)),
            SortField::Voltage => voltage_of(left).total_cmp(&voltage_of(right)),
            SortField::Firmware => firmware_of(left).cmp(&firmware_of(right)),
            SortField::Age => last_seen_of(left).cmp(&last_seen_of(right)),
        }
    }
}

// Pre-filter totals for the selected network, gathered by the same walk
// that builds the projection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WalkCounts {
    pub total: usize,
    pub online: usize,
    pub uncertain: usize,
    pub offline: usize,
}

// The tests exercise the walk through its list face.
#[cfg(test)]
impl NodeView<'_> {
    pub(crate) fn nodes<'e>(&self, engine: &'e EngineState) -> Vec<&'e Node> {
        self.walk(engine).0
    }

    pub(crate) fn keys(&self, engine: &EngineState) -> Vec<NodeKey> {
        self.nodes(engine).iter().map(|node| node.key).collect()
    }
}

fn name_of(node: &Node) -> &str {
    node.name.as_deref().unwrap_or("")
}

// Sink > head > sub > unknown: a small rank so sorting groups sinks first
// (descending) or pushes them to the bottom of an ascending sort.
fn role_rank(node: &Node) -> u8 {
    match node.role {
        Role::Sink => 3,
        Role::Headnode => 2,
        Role::Subnode => 1,
        Role::Unknown => 0,
    }
}

fn voltage_of(node: &Node) -> f32 {
    node.voltage.unwrap_or(f32::MIN)
}

fn firmware_of(node: &Node) -> [u32; 4] {
    node.firmware
        .map(|version| {
            [
                version.major,
                version.minor,
                version.maintenance,
                version.devel,
            ]
        })
        .unwrap_or([0; 4])
}

fn last_seen_of(node: &Node) -> u64 {
    node.last_seen_ms.unwrap_or(0)
}

// Rows rendered beyond the viewport so a one-line move never redraws a
// blank row; shared by the render window and the reducer's offset sync.
pub const OVERSCAN: usize = 10;

// Selection-index → visible window. Pure math so it tests table-driven.
// `previous` is the last scroll offset: the window only moves when the
// selection would leave it (scroll-under-the-cursor). Overscan pads the
// slice so a one-line move never redraws a blank row.
pub fn window(
    total: usize,
    selection: usize,
    height: usize,
    overscan: usize,
    previous: usize,
) -> Window {
    let height = height.max(1);

    if total == 0 {
        return Window { offset: 0, end: 0 };
    }

    let selection = selection.min(total - 1);
    let max_offset = total.saturating_sub(height);

    let offset = if selection < previous {
        selection
    } else if selection >= previous + height {
        selection + 1 - height
    } else {
        previous
    }
    .min(max_offset);

    Window {
        offset,
        end: (offset + height + overscan).min(total),
    }
}

// The visible slice of the projection: rows [offset, end) are rendered,
// with `overscan` extra rows below the viewport so a one-line move down
// never redraws a blank row. Moving up bumps the offset itself, so no
// overscan is needed above.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Window {
    // Index of the row painted at the top of the viewport.
    pub offset: usize,
    // Exclusive end of the render slice into the projection.
    pub end: usize,
}

pub fn selection_index_in(keys: &[NodeKey], selected: Option<NodeKey>) -> usize {
    selected
        .and_then(|key| keys.iter().position(|candidate| *candidate == key))
        .unwrap_or(0)
}

pub fn format_age(now_ms: u64, last_seen_ms: Option<u64>) -> String {
    let Some(seen) = last_seen_ms else {
        return "-".to_owned();
    };

    let seconds = now_ms.saturating_sub(seen) / 1000;

    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wntui_core::model::{FirmwareVersion, OnlineStatus};

    fn node(address: u32, name: &str, voltage: f32, firmware: [u32; 4]) -> Node {
        Node {
            key: NodeKey::new(42, address),
            name: Some(name.to_owned()),
            voltage: Some(voltage),
            firmware: Some(FirmwareVersion {
                major: firmware[0],
                minor: firmware[1],
                maintenance: firmware[2],
                devel: firmware[3],
            }),
            last_seen_ms: Some(1_700_000_000_000 + u64::from(address) * 1000),
            ..Node::default()
        }
    }

    fn role_node(address: u32, role: Role, online: OnlineStatus) -> Node {
        Node {
            role,
            online,
            ..node(address, "n", 3.0, [1, 0, 0, 0])
        }
    }

    fn engine_with(nodes: Vec<Node>) -> EngineState {
        let mut engine = EngineState::default();
        nodes.into_iter().for_each(|node| {
            engine.store.upsert(node);
        });
        engine
    }

    fn addresses(nodes: &[&Node]) -> Vec<u32> {
        nodes.iter().map(|node| node.key.address).collect()
    }

    #[test]
    fn sorting_cycles_through_every_field() {
        assert_eq!(SortField::Address.next(), SortField::Name);
        assert_eq!(SortField::Name.next(), SortField::Status);
        assert_eq!(SortField::Status.next(), SortField::Role);
        assert_eq!(SortField::Role.next(), SortField::Voltage);
        assert_eq!(SortField::Voltage.next(), SortField::Firmware);
        assert_eq!(SortField::Firmware.next(), SortField::Age);
        assert_eq!(SortField::Age.next(), SortField::Address);
    }

    #[test]
    fn sort_fields_parse_from_palette_words() {
        assert_eq!(SortField::parse("addr"), Some(SortField::Address));
        assert_eq!(SortField::parse(" Address "), Some(SortField::Address));
        assert_eq!(SortField::parse("status"), Some(SortField::Status));
        assert_eq!(SortField::parse("state"), Some(SortField::Status));
        assert_eq!(SortField::parse("role"), Some(SortField::Role));
        assert_eq!(SortField::parse("fw"), Some(SortField::Firmware));
        assert_eq!(SortField::parse("banana"), None);
    }

    #[test]
    fn nodes_sort_by_the_chosen_field_and_reverse() {
        let engine = engine_with(vec![
            node(3, "beta", 3.5, [4, 0, 0, 0]),
            node(1, "gamma", 3.0, [5, 0, 0, 0]),
            node(2, "alpha", 3.2, [3, 0, 0, 0]),
        ]);

        let view = NodeView {
            network: None,
            sort: SortField::Address,
            reverse: false,
            filter: None,
            now_ms: 0,
        };
        assert_eq!(addresses(&view.nodes(&engine)), vec![1, 2, 3]);

        let by_name = NodeView {
            sort: SortField::Name,
            ..view
        };
        assert_eq!(addresses(&by_name.nodes(&engine)), vec![2, 3, 1]);

        let by_voltage_desc = NodeView {
            sort: SortField::Voltage,
            reverse: true,
            ..view
        };
        assert_eq!(addresses(&by_voltage_desc.nodes(&engine)), vec![3, 2, 1]);

        let by_firmware_desc = NodeView {
            sort: SortField::Firmware,
            reverse: true,
            ..view
        };
        assert_eq!(addresses(&by_firmware_desc.nodes(&engine)), vec![1, 3, 2]);

        let by_age_desc = NodeView {
            sort: SortField::Age,
            reverse: true,
            ..view
        };
        assert_eq!(addresses(&by_age_desc.nodes(&engine)), vec![3, 2, 1]);
    }

    #[test]
    fn nodes_sort_by_status_and_role() {
        let engine = engine_with(vec![
            role_node(5, Role::Subnode, OnlineStatus::Online),
            role_node(1, Role::Sink, OnlineStatus::Offline),
            role_node(3, Role::Headnode, OnlineStatus::Uncertain),
            role_node(2, Role::Sink, OnlineStatus::Online),
            role_node(4, Role::Subnode, OnlineStatus::Offline),
        ]);

        let view = NodeView {
            network: None,
            sort: SortField::Status,
            reverse: false,
            filter: None,
            now_ms: 0,
        };

        // Ascending: offline < uncertain < online. Addresses within a group
        // follow the store's iteration order, so assert statuses.
        let statuses: Vec<OnlineStatus> =
            view.nodes(&engine).iter().map(|node| node.online).collect();
        assert_eq!(
            statuses,
            vec![
                OnlineStatus::Offline,
                OnlineStatus::Offline,
                OnlineStatus::Uncertain,
                OnlineStatus::Online,
                OnlineStatus::Online
            ]
        );

        let by_role_desc = NodeView {
            sort: SortField::Role,
            reverse: true,
            ..view
        };

        // Descending: sinks first, then head, then subs (address tiebreak
        // via the stable sort preserving insertion order is not guaranteed,
        // so assert the grouped roles instead of exact addresses).
        let roles: Vec<Role> = by_role_desc
            .nodes(&engine)
            .iter()
            .map(|node| node.role)
            .collect();
        assert_eq!(
            roles,
            vec![
                Role::Sink,
                Role::Sink,
                Role::Headnode,
                Role::Subnode,
                Role::Subnode
            ]
        );
    }

    #[test]
    fn selection_falls_back_to_the_first_node() {
        let engine = engine_with(vec![
            node(1, "a", 3.0, [1, 0, 0, 0]),
            node(2, "b", 3.1, [1, 0, 0, 0]),
        ]);
        let keys = NodeView {
            network: None,
            sort: SortField::Address,
            reverse: false,
            filter: None,
            now_ms: 0,
        }
        .keys(&engine);

        assert_eq!(selection_index_in(&keys, None), 0);
        assert_eq!(selection_index_in(&keys, Some(NodeKey::new(42, 2))), 1);
        assert_eq!(selection_index_in(&keys, Some(NodeKey::new(9, 9))), 0);
    }

    #[test]
    fn age_formats_compactly() {
        let now = 1_700_000_000_000;
        assert_eq!(format_age(now, Some(now - 5_000)), "5s");
        assert_eq!(format_age(now, Some(now - 65_000)), "1m");
        assert_eq!(format_age(now, Some(now - 7_200_000)), "2h");
        assert_eq!(format_age(now, Some(now - 3 * 86_400_000)), "3d");
        assert_eq!(format_age(now, None), "-");
    }

    #[test]
    fn window_is_sticky_and_clamped() {
        // (total, selection, height, overscan, previous, expected offset)
        let cases: &[(usize, usize, usize, usize, usize, usize)] = &[
            // Empty list: nothing to show.
            (0, 0, 10, 2, 0, 0),
            // Selection inside the window: offset unchanged (sticky).
            (100, 5, 10, 2, 3, 3),
            (100, 12, 10, 2, 3, 3),
            // Selection above the window: top follows the selection.
            (100, 2, 10, 2, 3, 2),
            (100, 0, 10, 2, 5, 0),
            // Selection below the window: selection lands on the last row.
            (100, 13, 10, 2, 3, 4),
            (100, 50, 10, 2, 3, 41),
            // End of the list: offset clamps to total - height.
            (100, 99, 10, 2, 3, 90),
            // List shorter than the viewport: offset stays at 0.
            (5, 4, 10, 2, 0, 0),
            // Zero height is guarded to one row: the selected row is the
            // whole window.
            (5, 4, 0, 2, 0, 4),
        ];

        for &(total, selection, height, overscan, previous, expected) in cases {
            let pane = window(total, selection, height, overscan, previous);
            assert_eq!(
                pane.offset, expected,
                "total={total} selection={selection} height={height} previous={previous}"
            );
            assert!(pane.offset <= pane.end);
        }
    }

    #[test]
    fn window_slices_include_overscan_but_never_leave_bounds() {
        // The slice covers the viewport plus overscan below it.
        let pane = window(100, 50, 10, 10, 41);
        assert_eq!(pane.offset, 41);
        assert_eq!(pane.end, 61);
        assert_eq!(pane.end - pane.offset, 20);

        // Near the top the slice starts at the offset (0).
        let pane = window(100, 3, 10, 10, 0);
        assert_eq!(pane.offset, 0);
        assert_eq!(pane.end, 20);

        // Near the bottom it clamps at total.
        let pane = window(100, 99, 10, 10, 90);
        assert_eq!(pane.offset, 90);
        assert_eq!(pane.end, 100);

        // Overscan cannot push the slice past the list end.
        let pane = window(5, 4, 10, 10, 0);
        assert_eq!(pane.end, 5);
    }

    // ProjectionInputs borrows the filter, so tests own the filter and
    // lend the inputs on each refresh.
    struct OwnedInputs {
        filter_text: String,
        filter: Option<wntui_core::filter::Filter>,
        sort: SortField,
        network: Option<u32>,
        now_ms: u64,
    }

    impl OwnedInputs {
        fn new() -> Self {
            Self {
                filter_text: String::new(),
                filter: None,
                sort: SortField::Address,
                network: None,
                now_ms: 0,
            }
        }

        fn parse(filter_text: &str) -> Self {
            Self {
                filter_text: filter_text.to_owned(),
                filter: wntui_core::filter::parse(filter_text).ok(),
                ..Self::new()
            }
        }

        fn refresh(&self, projection: &mut Projection, engine: &EngineState) {
            let inputs = ProjectionInputs {
                filter: self.filter.as_ref(),
                filter_text: &self.filter_text,
                sort: self.sort,
                reverse: false,
                network: self.network,
                now_ms: self.now_ms,
            };
            projection.refresh(engine, &inputs);
        }
    }

    fn big_engine(count: usize) -> EngineState {
        let mut engine = EngineState::default();
        for address in 1..=count {
            let key = NodeKey::new(42, address as u32);
            engine.store.upsert(Node {
                key,
                name: Some(format!("node {address}")),
                online: OnlineStatus::Online,
                ..Node::default()
            });
        }
        engine
    }

    #[test]
    fn projection_recomputes_only_when_an_input_moves() {
        let mut engine = big_engine(50);
        let inputs = OwnedInputs::new();

        let mut projection = Projection::default();
        inputs.refresh(&mut projection, &engine);
        let first = projection.recomputes;
        assert_eq!(projection.keys().len(), 50);

        // Same inputs: cached, no recompute.
        inputs.refresh(&mut projection, &engine);
        assert_eq!(projection.recomputes, first);

        // Store mutation: recompute.
        engine.store.upsert(Node {
            key: NodeKey::new(42, 999),
            ..Node::default()
        });
        inputs.refresh(&mut projection, &engine);
        assert_eq!(projection.recomputes, first + 1);
        assert_eq!(projection.keys().len(), 51);

        // Filter change: recompute.
        let filtered = OwnedInputs::parse("status:online");
        filtered.refresh(&mut projection, &engine);
        assert_eq!(projection.recomputes, first + 2);

        // Sort change: recompute.
        let sorted = OwnedInputs {
            sort: SortField::Name,
            ..OwnedInputs::new()
        };
        sorted.refresh(&mut projection, &engine);
        assert_eq!(projection.recomputes, first + 3);

        // Network change: recompute.
        let networked = OwnedInputs {
            network: Some(7),
            ..OwnedInputs::new()
        };
        networked.refresh(&mut projection, &engine);
        assert_eq!(projection.recomputes, first + 4);
        assert!(projection.keys().is_empty());
        assert_eq!(projection.total(), 0);
    }

    #[test]
    fn projection_totals_count_pre_filter_nodes() {
        let mut engine = big_engine(10);
        // Two not-online, one on another network.
        engine.store.upsert(Node {
            key: NodeKey::new(42, 11),
            online: OnlineStatus::Offline,
            ..Node::default()
        });
        engine.store.upsert(Node {
            key: NodeKey::new(42, 12),
            online: OnlineStatus::Uncertain,
            ..Node::default()
        });
        engine.store.upsert(Node {
            key: NodeKey::new(7, 1),
            ..Node::default()
        });

        let mut inputs = OwnedInputs::parse("status:online");
        inputs.network = Some(42);
        let mut projection = Projection::default();
        inputs.refresh(&mut projection, &engine);

        assert_eq!(projection.len(), 10);
        assert_eq!(projection.total(), 12);
        assert_eq!(projection.status_counts(), (10, 1, 1));
    }

    #[test]
    fn time_dependent_filters_recompute_once_per_second() {
        let engine = big_engine(5);
        let mut inputs = OwnedInputs::parse("age:>10s");
        inputs.now_ms = 100_000;

        let mut projection = Projection::default();
        inputs.refresh(&mut projection, &engine);
        let first = projection.recomputes;

        // Same second: cached.
        inputs.now_ms = 100_999;
        inputs.refresh(&mut projection, &engine);
        assert_eq!(projection.recomputes, first);

        // Next second: recompute.
        inputs.now_ms = 101_000;
        inputs.refresh(&mut projection, &engine);
        assert_eq!(projection.recomputes, first + 1);
    }

    #[test]
    fn huge_networks_scroll_a_bounded_window() {
        // 100k nodes against a small viewport: the offset the reducer
        // stores must cover only the visible slice plus overscan, never
        // the network size.
        let engine = big_engine(100_000);
        let mut app = AppState::new();
        app.selected = Some(NodeKey::new(42, 1));
        app.page_height = 8;
        app.now_ms = 0;

        crate::tui::update::refresh_projection(&mut app, &engine);

        assert_eq!(app.projection.len(), 100_000);
        assert_eq!(app.scroll_offset, 0);

        // Jump to the last row: the offset clamps at total - height, the
        // window stays 8 rows wide.
        app.selected = Some(NodeKey::new(42, 100_000));
        crate::tui::update::refresh_projection(&mut app, &engine);
        assert_eq!(app.scroll_offset, 100_000 - 8);

        let pane = window(
            app.projection.len(),
            100_000 - 1,
            8,
            OVERSCAN,
            app.scroll_offset,
        );
        assert!(pane.end - pane.offset <= 8 + OVERSCAN);
    }
}
