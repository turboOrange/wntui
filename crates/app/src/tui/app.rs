use std::{collections::BTreeMap, fmt};

use wntui_core::{
    filter::{Filter, LocatedParseError},
    model::NodeKey,
    wire::{
        influx::{GraphMetric, GraphRange, Series},
        metadata::NodeMetadataUpdate,
    },
};

use super::listing::{Projection, SortField};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ConnectionState {
    #[default]
    Connecting,
    Syncing,
    Live,
    Reconnecting,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Page {
    #[default]
    Nodes,
    Networks,
    NodeDetail,
    Graphs,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overlay {
    Help,
    Palette,
    Filter,
    Fields,
    Edit(EditField),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditField {
    Name,
    Description,
}

impl EditField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Description => "description",
        }
    }
}

#[derive(Clone, Debug)]
pub struct EditDraft {
    pub key: NodeKey,
    pub field: EditField,
    pub original: String,
    pub input: String,
}

impl fmt::Display for Page {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Nodes => "NODES",
            Self::Networks => "NETWORKS",
            Self::NodeDetail => "NODE",
            Self::Graphs => "GRAPHS",
        };
        formatter.write_str(label)
    }
}

// One in-flight or landed graph fetch. Fetches are keyed by
// (metric, range/window); a stale reply for an old request is dropped on
// arrival by matching the echoed window.
#[derive(Clone, Debug)]
pub struct GraphRequest {
    pub key: NodeKey,
    pub metric: GraphMetric,
    pub range: GraphRange,
    // The window this request asked for: None = the range, Some = an
    // absolute (start, end) in unix seconds. The reply echoes it.
    pub requested: Option<(i64, i64)>,
    // The time bounds the data on hand covers: range-derived or the
    // requested window. Pan/zoom only refetches when the new window
    // would leave it (the zoom fast path, N4).
    pub fetched: (f64, f64),
    // The displayed window; None = the whole fetched span.
    pub window: Option<(f64, f64)>,
    pub series: Vec<Series>,
    pub loading: bool,
    pub error: Option<String>,
}

impl GraphRequest {
    pub fn new(key: NodeKey, metric: GraphMetric, range: GraphRange) -> Self {
        Self {
            key,
            metric,
            range,
            requested: None,
            fetched: (0.0, 0.0),
            window: None,
            series: Vec::new(),
            loading: true,
            error: None,
        }
    }
}

// A fetch the runtime loop still has to forward to the engine, as data:
// the reducer stays side-effect free (Elm/TEA).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingGraph {
    pub key: NodeKey,
    pub metric: GraphMetric,
    pub range: GraphRange,
    pub window: Option<(i64, i64)>,
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub page: Page,
    pub overlay: Option<Overlay>,
    pub palette_input: String,
    pub filter_input: String,
    pub filter_text: String,
    pub filter: Option<Filter>,
    // Live parse state of filter_input while the filter overlay is open:
    // the error (with caret span) or None when the expression is valid.
    pub filter_error: Option<LocatedParseError>,
    // Tab-completion suggestions for the current filter_input.
    pub filter_suggestion: Option<String>,
    // Named filter profiles loaded from config; `:filter-save` writes back
    // through `pending_filter_save`.
    pub saved_filters: BTreeMap<String, String>,
    pub pending_filter_save: Option<(String, String)>,
    pub selected_network: Option<u32>,
    pub selected: Option<NodeKey>,
    pub picker_row: usize,
    pub net_query: String,
    pub detail_cursor: usize,
    // Cached filter+sort projection of the node list; refreshed by the
    // runtime loop before each frame (N2).
    pub projection: Projection,
    // Scroll offset of the node table in projection rows.
    pub scroll_offset: usize,
    pub sort: SortField,
    pub sort_reverse: bool,
    pub page_height: usize,
    pub now_ms: u64,
    pub message: Option<String>,
    pub edit: Option<EditDraft>,
    pub pending_edit: Option<NodeMetadataUpdate>,
    pub graph: Option<GraphRequest>,
    pub pending_graph: Option<PendingGraph>,
    pub connection: ConnectionState,
    pub quit: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            page: Page::Nodes,
            overlay: None,
            palette_input: String::new(),
            filter_input: String::new(),
            filter_text: String::new(),
            filter: None,
            filter_error: None,
            filter_suggestion: None,
            saved_filters: BTreeMap::new(),
            pending_filter_save: None,
            selected_network: None,
            selected: None,
            picker_row: 0,
            net_query: String::new(),
            detail_cursor: 0,
            projection: Projection::default(),
            scroll_offset: 0,
            sort: SortField::Address,
            sort_reverse: false,
            page_height: 20,
            now_ms: 0,
            message: None,
            edit: None,
            pending_edit: None,
            graph: None,
            pending_graph: None,
            connection: ConnectionState::default(),
            quit: false,
        }
    }

    pub fn mode_label(&self) -> &'static str {
        match self.overlay {
            Some(Overlay::Palette) => "PALETTE",
            Some(Overlay::Filter) => "FILTER",
            Some(Overlay::Fields) => "FIELDS",
            Some(Overlay::Help) => "HELP",
            Some(Overlay::Edit(_)) => "EDIT",
            None => match self.page {
                Page::Nodes => "NORMAL",
                Page::Networks => "NETWORKS",
                Page::NodeDetail => "NODE",
                Page::Graphs => "GRAPHS",
            },
        }
    }

    // Absorbs an engine event into the app state. Graph replies only land
    // when their (metric, range/window) matches the current inflight
    // request; stale replies from a previous range or window are dropped.
    pub fn apply_engine_event(&mut self, event: wntui_core::engine::EngineEvent) {
        use wntui_core::engine::EngineEvent;

        match event {
            EngineEvent::GraphData {
                key,
                metric,
                range,
                window,
                series,
            } => {
                if let Some(graph) = self.graph.as_mut()
                    && graph.key == key
                    && graph.metric == metric
                    && graph.range == range
                    && graph.requested == window
                {
                    graph.series = series;
                    graph.loading = false;
                    graph.error = None;
                }
            }
            EngineEvent::GraphFailed {
                key,
                metric,
                range,
                window,
                reason,
            } => {
                if let Some(graph) = self.graph.as_mut()
                    && graph.key == key
                    && graph.metric == metric
                    && graph.range == range
                    && graph.requested == window
                {
                    graph.loading = false;
                    graph.error = Some(reason);
                }
            }
            EngineEvent::MetadataEditAccepted { .. } | EngineEvent::MetadataEditFailed { .. } => {}
        }
    }
}
