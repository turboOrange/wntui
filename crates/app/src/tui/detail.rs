use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use wntui_core::{
    engine::EngineState,
    model::{Node, OnlineStatus, Role},
    wire::influx::GraphMetric,
};

use crate::tui::theme::{self as color};

// A detail row's affordance (review F6): plain rows are read-only,
// editable rows open the name/description editor, graphable rows open
// the graphs page for their metric.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DetailField {
    Normal,
    Editable(EditKind),
    Graphable(GraphMetric),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Name,
    Description,
}

#[derive(Clone, Debug)]
pub struct DetailRow {
    pub text: Line<'static>,
    pub field: DetailField,
}

impl DetailRow {
    pub fn style(&self) -> Style {
        match self.field {
            DetailField::Normal => Style::default(),
            // White keeps the value readable on the saturated affordance
            // backgrounds; the row's own spans keep only their content.
            DetailField::Editable(_) => Style::default()
                .bg(color::EDITABLE_BG)
                .fg(color::AFFORDANCE_FG),
            DetailField::Graphable(_) => Style::default()
                .bg(color::GRAPH_BG)
                .fg(color::AFFORDANCE_FG),
        }
    }
}

pub fn network_label(engine: &EngineState, selected: Option<u32>) -> String {
    match selected {
        None => "all networks".to_owned(),
        Some(id) => match engine.networks.iter().find(|network| network.id == id) {
            Some(network) if !network.name.is_empty() => format!("{} ({id})", network.name),
            _ => format!("network {id}"),
        },
    }
}

pub fn status_word(status: OnlineStatus) -> &'static str {
    match status {
        OnlineStatus::Online => "online",
        OnlineStatus::Uncertain => "uncertain",
        OnlineStatus::Offline => "offline",
    }
}

fn role_word(role: Role) -> &'static str {
    match role {
        Role::Sink => "sink",
        Role::Headnode => "headnode",
        Role::Subnode => "subnode",
        Role::Unknown => "unknown",
    }
}

fn bool_word(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "-",
    }
}

fn position_label(node: &Node) -> String {
    match (node.latitude, node.longitude) {
        (Some(latitude), Some(longitude)) => format!(
            "{latitude:.6}, {longitude:.6}{}",
            node.altitude
                .map(|altitude| format!(", {altitude:.1} m"))
                .unwrap_or_default()
        ),
        _ => "-".to_owned(),
    }
}

fn unix_time_label(seconds: u32) -> String {
    let days = seconds / 86_400;
    let time_of_day = seconds % 86_400;
    format!(
        "{days}d {:02}:{:02}:{:02}",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60
    )
}

fn stack_profile_label(profile: u32) -> String {
    let label = match profile {
        1 => "2.4 GHz",
        2 => "868 MHz",
        3 => "915 MHz",
        4 => "870 MHz",
        5 => "917 MHz",
        8 => "865 MHz",
        9 => "EFR32 2.4 GHz",
        11 => "915 MHz (AU)",
        12 => "DECT-NR",
        13 => "nRF52 2.4 GHz (+4 dBm)",
        14 => "nRF52 2.4 GHz (+8 dBm)",
        15 => "DECT-NR band 4",
        20 => "DECT-NR band 9",
        _ => return format!("profile {profile}"),
    };
    label.to_owned()
}

fn hardware_label(hardware: u32) -> String {
    let label = match hardware {
        1 => "nRF51",
        2 => "EZR32",
        3 => "nRF52",
        4 => "CC2650",
        5 => "EFR32XG12 1M",
        6 => "nRF52840",
        7 => "EFR32XG12 512k",
        8 => "EFR32XG13",
        9 => "nRF52833",
        10 => "EFR32XG21",
        11 => "EFR32XG22",
        15 => "nRF9160",
        16 => "EFR32XG23",
        17 => "EFR32XG24",
        18 => "nRF9120",
        _ => return format!("hw {hardware}"),
    };
    label.to_owned()
}

fn boot_reason_label(reason: u32) -> String {
    let label = match reason {
        0 => "power-on reset",
        1 => "requested reboot",
        2 => "software assert",
        3 => "MCU fault",
        4 => "watchdog reset",
        _ => return format!("reason {reason}"),
    };
    label.to_owned()
}

fn detail_row(label: &str, value: &str, field: DetailField) -> DetailRow {
    DetailRow {
        text: Line::from(vec![
            Span::styled(format!("   {label:<22}"), Style::default().fg(color::DIM)),
            Span::raw(value.to_owned()),
        ]),
        field,
    }
}

fn section(title: &'static str, rows: &mut Vec<DetailRow>, entries: Vec<DetailRow>) {
    rows.push(DetailRow {
        text: Line::styled(format!(" {title}"), Style::default().fg(color::PRIMARY)),
        field: DetailField::Normal,
    });
    rows.extend(entries);
    rows.push(DetailRow {
        text: Line::raw(""),
        field: DetailField::Normal,
    });
}

fn title_row(node: &Node) -> DetailRow {
    DetailRow {
        text: Line::from(vec![
            Span::styled(
                format!(
                    " {}",
                    node.name.clone().unwrap_or_else(|| "node".to_owned())
                ),
                Style::default()
                    .fg(color::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    "{} {}",
                    color::status_glyph(node.online),
                    status_word(node.online)
                ),
                Style::default().fg(color::status_color(node.online)),
            ),
        ]),
        field: DetailField::Normal,
    }
}

// The node detail page as a pure row projection: the view renders it,
// the reducer navigates it (cursor clamped to len) and maps the field
// under the cursor to an action.
pub fn detail_rows(node: &Node, engine: &EngineState, now_ms: u64) -> Vec<DetailRow> {
    let mut rows = vec![
        title_row(node),
        DetailRow {
            text: Line::raw(""),
            field: DetailField::Normal,
        },
    ];

    section(
        "identity",
        &mut rows,
        vec![
            detail_row(
                "network",
                &network_label(engine, Some(node.key.network_id)),
                DetailField::Normal,
            ),
            detail_row(
                "address",
                &format!("0x{:04x} ({})", node.key.address, node.key.address),
                DetailField::Normal,
            ),
            detail_row(
                "name",
                node.name.as_deref().unwrap_or("-"),
                DetailField::Editable(EditKind::Name),
            ),
            detail_row(
                "description",
                node.description.as_deref().unwrap_or("-"),
                DetailField::Editable(EditKind::Description),
            ),
            detail_row("approved", bool_word(node.approved), DetailField::Normal),
            detail_row("virtual", bool_word(node.is_virtual), DetailField::Normal),
        ],
    );

    section(
        "status",
        &mut rows,
        vec![
            detail_row(
                "online",
                status_word(node.online),
                DetailField::Graphable(GraphMetric::Online),
            ),
            detail_row(
                "previous",
                node.previous_online.map(status_word).unwrap_or("-"),
                DetailField::Normal,
            ),
            detail_row(
                "since",
                &node
                    .online_updated_s
                    .map(unix_time_label)
                    .unwrap_or_else(|| "-".to_owned()),
                DetailField::Normal,
            ),
            detail_row(
                "last seen",
                &crate::tui::listing::format_age(now_ms, node.last_seen_ms),
                DetailField::Normal,
            ),
            detail_row(
                "gateway",
                node.gateway_id.as_deref().unwrap_or("-"),
                DetailField::Normal,
            ),
        ],
    );

    let mut mesh_rows = vec![
        detail_row("role", role_word(node.role), DetailField::Normal),
        detail_row(
            "latency mode",
            &match node.cb_mac {
                Some(true) => "low latency".to_owned(),
                Some(false) => "low energy".to_owned(),
                None => "-".to_owned(),
            },
            DetailField::Normal,
        ),
        detail_row("autorole", bool_word(node.is_autorole), DetailField::Normal),
        detail_row(
            "sink",
            &node
                .sink_address
                .map(|address| format!("0x{address:04x}"))
                .unwrap_or_else(|| "-".to_owned()),
            DetailField::Normal,
        ),
    ];

    if !node.neighbors.is_empty() {
        let rendered = node
            .neighbors
            .iter()
            .map(|neighbor| {
                format!(
                    "0x{:04x} {:.0} dBm{}",
                    neighbor.address,
                    neighbor.rssi_dbm,
                    if neighbor.is_next_hop {
                        " (next hop)"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        mesh_rows.push(detail_row("neighbors", &rendered, DetailField::Normal));
    }

    section("mesh", &mut rows, mesh_rows);

    section(
        "power & radio",
        &mut rows,
        vec![
            detail_row(
                "voltage",
                &node
                    .voltage
                    .map(|voltage| format!("{voltage:.2} V"))
                    .unwrap_or_else(|| "-".to_owned()),
                DetailField::Graphable(GraphMetric::Voltage),
            ),
            detail_row(
                "stack profile",
                &node
                    .stack_profile
                    .map(stack_profile_label)
                    .unwrap_or_else(|| "-".to_owned()),
                DetailField::Normal,
            ),
        ],
    );

    section(
        "firmware",
        &mut rows,
        vec![
            detail_row(
                "stack",
                &node
                    .firmware
                    .map(|version| version.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                DetailField::Normal,
            ),
            detail_row(
                "hardware",
                &node
                    .hardware
                    .map(hardware_label)
                    .unwrap_or_else(|| "-".to_owned()),
                DetailField::Normal,
            ),
            detail_row(
                "boot count",
                &node
                    .boot_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                DetailField::Graphable(GraphMetric::Boots),
            ),
            detail_row(
                "last boot reason",
                &node
                    .boot_reason
                    .map(boot_reason_label)
                    .unwrap_or_else(|| "-".to_owned()),
                DetailField::Normal,
            ),
            detail_row(
                "otap enabled",
                bool_word(node.otap_enabled),
                DetailField::Normal,
            ),
            detail_row(
                "scratchpad seq",
                &node
                    .scratchpad_sequence
                    .map(|sequence| sequence.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                DetailField::Normal,
            ),
        ],
    );

    section(
        "positioning",
        &mut rows,
        vec![
            detail_row(
                "role",
                &match node.positioning_role {
                    Some(wntui_core::model::PositioningRole::Anchor) => "anchor".to_owned(),
                    Some(wntui_core::model::PositioningRole::Tag) => "tag".to_owned(),
                    _ => "-".to_owned(),
                },
                DetailField::Normal,
            ),
            detail_row(
                "rssi offset",
                &node
                    .rssi_offset
                    .map(|offset| offset.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                DetailField::Normal,
            ),
            detail_row("position", &position_label(node), DetailField::Normal),
        ],
    );

    if let Some(app_config) = node.app_config {
        section(
            "app config (sink)",
            &mut rows,
            vec![
                detail_row(
                    "diagnostics interval",
                    &format!("{} s", app_config.diagnostics_interval_s),
                    DetailField::Normal,
                ),
                detail_row(
                    "sequence",
                    &app_config.sequence.to_string(),
                    DetailField::Normal,
                ),
                detail_row(
                    "override",
                    &app_config.is_override_on.to_string(),
                    DetailField::Normal,
                ),
            ],
        );
    }

    if let Some(kpi) = node.traveltime_kpi {
        section(
            "travel time",
            &mut rows,
            vec![
                detail_row(
                    "qos 0",
                    &format!(
                        "min {:.0} · avg {:.0} · max {:.0} ms",
                        kpi.qos0_minimum_ms, kpi.qos0_average_ms, kpi.qos0_maximum_ms
                    ),
                    DetailField::Graphable(GraphMetric::TraveltimeQos0),
                ),
                detail_row(
                    "qos 1",
                    &format!(
                        "min {:.0} · avg {:.0} · max {:.0} ms",
                        kpi.qos1_minimum_ms, kpi.qos1_average_ms, kpi.qos1_maximum_ms
                    ),
                    DetailField::Graphable(GraphMetric::TraveltimeQos1),
                ),
                detail_row(
                    "window",
                    &format!("{} s", kpi.window_s),
                    DetailField::Normal,
                ),
            ],
        );
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use wntui_core::model::NodeKey;

    fn engine() -> EngineState {
        let mut engine = EngineState::default();
        engine.networks = vec![wntui_core::model::Network {
            id: 42,
            name: "Demo plant".to_owned(),
        }];
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));
        engine
    }

    fn rows_for(address: u32, engine: &EngineState) -> Vec<DetailRow> {
        let node = engine.store.node(&NodeKey::new(42, address)).unwrap();
        detail_rows(node, engine, 1_700_000_005_000)
    }

    fn row_text(row: &DetailRow) -> String {
        row.text
            .spans
            .iter()
            .map(|span| span.content.to_string())
            .collect()
    }

    #[test]
    fn editable_and_graphable_fields_are_marked() {
        let engine = engine();
        let rows = rows_for(2, &engine);

        let fields: Vec<DetailField> = rows.iter().map(|row| row.field).collect();
        assert!(fields.contains(&DetailField::Editable(EditKind::Name)));
        assert!(fields.contains(&DetailField::Editable(EditKind::Description)));
        assert!(fields.contains(&DetailField::Graphable(GraphMetric::Voltage)));
    }

    #[test]
    fn affordance_styles_distinct_backgrounds() {
        let editable = detail_row("name", "x", DetailField::Editable(EditKind::Name));
        let graphable = detail_row(
            "voltage",
            "3.1 V",
            DetailField::Graphable(GraphMetric::Voltage),
        );
        let plain = detail_row("role", "sink", DetailField::Normal);

        assert_eq!(editable.style().bg, Some(color::EDITABLE_BG));
        assert_eq!(graphable.style().bg, Some(color::GRAPH_BG));
        assert_eq!(plain.style().bg, None);
        assert_eq!(
            editable.style().fg,
            Some(color::AFFORDANCE_FG),
            "white text on the affordance backgrounds"
        );
        assert_eq!(graphable.style().fg, Some(color::AFFORDANCE_FG));
        assert_eq!(plain.style().fg, None);
    }

    #[test]
    fn sink_rows_cover_every_section() {
        let engine = engine();
        let rows = rows_for(1, &engine);
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("identity"));
        assert!(text.contains("app config (sink)"));
        assert!(text.contains("positioning"));
        assert!(rows.len() > 30);
    }

    #[test]
    fn subnode_omits_sink_only_sections() {
        let engine = engine();
        let sink = rows_for(1, &engine);
        let sink_text = sink.iter().map(row_text).collect::<Vec<_>>().join("\n");
        let sub_text = rows_for(4, &engine)
            .iter()
            .map(row_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(sink_text.contains("app config (sink)"));
        assert!(
            !sub_text.contains("app config (sink)"),
            "the app config section is sink-only"
        );

        // Node 4 carries a traveltime section that offsets the missing app
        // config rows; node 5 has neither, so it is strictly shorter.
        let plain_sub = rows_for(5, &engine).len();
        assert!(plain_sub < sink.len());
    }

    #[test]
    fn traveltime_rows_are_graphable_when_present() {
        let engine = engine();
        let rows = rows_for(2, &engine);

        assert!(
            rows.iter()
                .any(|row| row.field == DetailField::Graphable(GraphMetric::TraveltimeQos0))
        );
    }
}
