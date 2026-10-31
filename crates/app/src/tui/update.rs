use wntui_core::{
    engine::EngineState,
    model::NodeKey,
    session::UserRole,
    wire::{
        influx::GraphMetric,
        metadata::{NodeMetadataPatch, NodeMetadataUpdate},
    },
};

use super::{
    app::{AppState, EditDraft, EditField, GraphRequest, Overlay, Page, PendingGraph},
    detail::{DetailField, EditKind, detail_rows},
    input::Action,
    listing::{SortField, selection_index_in},
    palette::{parse_command, resolve_network},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Continue,
    Quit,
}

pub fn update(state: &mut AppState, action: Action, engine: &EngineState) -> Outcome {
    toast_connection_change(state, engine);
    refresh_projection(state, engine);
    let outcome = step(state, action, engine);
    clamp_detail_cursor(state, engine);
    outcome
}

// The cache lives inside AppState but refresh needs &AppState (for the
// filter reference), so it is swapped out, refreshed, and put back.
// The node-list scroll offset is derived here too: render is pure and
// re-derives the same window from the stored offset.
pub fn refresh_projection(state: &mut AppState, engine: &EngineState) {
    let mut cache = std::mem::take(&mut state.projection);
    let inputs = super::listing::ProjectionInputs::from_app(state);
    cache.refresh(engine, &inputs);
    state.projection = cache;

    let keys = state.projection.keys();
    let selection = super::listing::selection_index_in(keys, state.selected);
    let pane = super::listing::window(
        keys.len(),
        selection,
        state.page_height,
        super::listing::OVERSCAN,
        state.scroll_offset,
    );
    state.scroll_offset = pane.offset;
}

// The cursor clamps to the row count every step: node updates can shrink
// the detail projection under the cursor at any time.
fn clamp_detail_cursor(state: &mut AppState, engine: &EngineState) {
    if state.page != Page::NodeDetail {
        return;
    }

    let count = detail_row_count(state, engine);
    if state.detail_cursor >= count {
        state.detail_cursor = count.saturating_sub(1);
    }
}

// Connection transitions surface as status-bar toasts so a dropped link
// is visible from any page (review: reconnect/error toasts).
fn toast_connection_change(state: &mut AppState, engine: &EngineState) {
    use super::app::ConnectionState as UiState;

    let observed = match engine.connection() {
        wntui_core::engine::ConnectionState::Connecting => UiState::Connecting,
        wntui_core::engine::ConnectionState::Reconnecting => UiState::Reconnecting,
        wntui_core::engine::ConnectionState::Syncing => UiState::Syncing,
        wntui_core::engine::ConnectionState::Live => UiState::Live,
    };

    if state.connection != observed {
        state.connection = observed;
        state.message = Some(match observed {
            UiState::Connecting => "connecting to the backend…".to_owned(),
            UiState::Reconnecting => "connection lost — reconnecting…".to_owned(),
            UiState::Syncing => "connected — syncing nodes…".to_owned(),
            UiState::Live => "live".to_owned(),
        });
    }
}

fn step(state: &mut AppState, action: Action, engine: &EngineState) -> Outcome {
    match action {
        Action::Quit => {
            state.quit = true;
            Outcome::Quit
        }
        Action::Noop => Outcome::Continue,

        Action::OpenPalette => {
            state.overlay = Some(Overlay::Palette);
            state.palette_input.clear();
            Outcome::Continue
        }
        Action::OpenFilter => {
            state.overlay = Some(Overlay::Filter);
            state.filter_input = state.filter_text.clone();
            refresh_filter_parse(state);
            Outcome::Continue
        }
        Action::OpenHelp => {
            state.overlay = Some(Overlay::Help);
            Outcome::Continue
        }
        Action::OpenNetworks => {
            state.page = Page::Networks;
            state.picker_row = 0;
            state.message = None;
            Outcome::Continue
        }
        Action::Back => {
            if state.overlay.take().is_none() {
                match state.page {
                    Page::Graphs => state.page = Page::NodeDetail,
                    Page::Networks | Page::NodeDetail => state.page = Page::Nodes,
                    Page::Nodes => {}
                }
            }
            Outcome::Continue
        }

        Action::MoveDown => {
            move_selection(state, engine, 1);
            Outcome::Continue
        }
        Action::MoveUp => {
            move_selection(state, engine, -1);
            Outcome::Continue
        }
        Action::ToTop => {
            if state.page == Page::NodeDetail {
                state.detail_cursor = 0;
            } else if state.page == Page::Networks {
                state.picker_row = 0;
            } else {
                state.selected = state.projection.keys().first().copied();
            }
            Outcome::Continue
        }
        Action::ToBottom => {
            if state.page == Page::NodeDetail {
                let count = detail_row_count(state, engine);
                state.detail_cursor = count.saturating_sub(1);
            } else if state.page == Page::Networks {
                let count = network_matches(engine, &state.net_query).len();
                state.picker_row = count.saturating_sub(1);
            } else {
                state.selected = state.projection.keys().last().copied();
            }
            Outcome::Continue
        }
        Action::HalfPageDown => {
            move_selection(state, engine, state.page_height as isize / 2);
            Outcome::Continue
        }
        Action::HalfPageUp => {
            move_selection(state, engine, -(state.page_height as isize / 2));
            Outcome::Continue
        }
        Action::FullPageDown => {
            move_selection(state, engine, state.page_height as isize);
            Outcome::Continue
        }
        Action::FullPageUp => {
            move_selection(state, engine, -(state.page_height as isize));
            Outcome::Continue
        }
        Action::ScrollUp => {
            // On the graphs page the wheel pans the window (N4); on the
            // list pages it scrolls the selection as before.
            if state.page == Page::Graphs {
                pan_graph(state, -1.0);
            } else {
                move_selection(state, engine, -3);
            }
            Outcome::Continue
        }
        Action::ScrollDown => {
            if state.page == Page::Graphs {
                pan_graph(state, 1.0);
            } else {
                move_selection(state, engine, 3);
            }
            Outcome::Continue
        }

        Action::CycleSort => {
            state.sort = state.sort.next();
            state.message = Some(format!("sorted by {}", state.sort.label()));
            Outcome::Continue
        }
        Action::ReverseSort => {
            state.sort_reverse = !state.sort_reverse;
            let direction = if state.sort_reverse {
                "descending"
            } else {
                "ascending"
            };
            state.message = Some(format!("sorted by {} ({direction})", state.sort.label()));
            Outcome::Continue
        }

        Action::CycleEditField => {
            if let Some(Overlay::Edit(current)) = state.overlay {
                let next = match current {
                    EditField::Name => EditField::Description,
                    EditField::Description => EditField::Name,
                };
                rotate_edit_draft(state, engine, next);
            }
            Outcome::Continue
        }

        Action::OpenGraphs => {
            open_graphs(state, engine);
            Outcome::Continue
        }
        Action::CycleGraphRange => {
            cycle_graph_range(state, engine);
            Outcome::Continue
        }
        Action::PanGraphLeft => {
            pan_graph(state, -1.0);
            Outcome::Continue
        }
        Action::PanGraphRight => {
            pan_graph(state, 1.0);
            Outcome::Continue
        }
        Action::ZoomInGraph => {
            zoom_graph(state, 0.5);
            Outcome::Continue
        }
        Action::ZoomOutGraph => {
            zoom_graph(state, 2.0);
            Outcome::Continue
        }
        Action::GraphJumpStart => {
            jump_graph(state, true);
            Outcome::Continue
        }
        Action::GraphJumpEnd => {
            jump_graph(state, false);
            Outcome::Continue
        }
        Action::RefetchGraph => {
            refetch_graph(state);
            Outcome::Continue
        }

        Action::Enter => {
            if state.overlay.is_none() {
                match state.page {
                    Page::Networks => select_highlighted_network(state, engine),
                    Page::Nodes => {
                        let highlighted = state
                            .selected
                            .or_else(|| state.projection.keys().first().copied());

                        if let Some(key) = highlighted {
                            state.selected = Some(key);
                            state.page = Page::NodeDetail;
                            state.detail_cursor = 0;
                        }
                    }
                    Page::NodeDetail => activate_detail_field(state, engine),
                    Page::Graphs => {}
                }
            }
            Outcome::Continue
        }

        Action::TextualInput(character) => {
            match state.overlay {
                Some(Overlay::Palette) => state.palette_input.push(character),
                Some(Overlay::Filter) => {
                    state.filter_input.push(character);
                    refresh_filter_parse(state);
                }
                Some(Overlay::Edit(_)) => {
                    if let Some(edit) = state.edit.as_mut() {
                        edit.input.push(character);
                    }
                }
                None if state.page == Page::Networks => {
                    state.net_query.push(character);
                    state.picker_row = 0;
                }
                _ => {}
            }
            Outcome::Continue
        }
        Action::TextualBackspace => {
            match state.overlay {
                Some(Overlay::Palette) => {
                    state.palette_input.pop();
                }
                Some(Overlay::Filter) => {
                    state.filter_input.pop();
                    refresh_filter_parse(state);
                }
                Some(Overlay::Edit(_)) => {
                    if let Some(edit) = state.edit.as_mut() {
                        edit.input.pop();
                    }
                }
                None if state.page == Page::Networks => {
                    state.net_query.pop();
                    state.picker_row = 0;
                }
                _ => {}
            }
            Outcome::Continue
        }
        Action::TextualCancel => {
            state.overlay = None;
            state.edit = None;
            state.filter_suggestion = None;
            Outcome::Continue
        }
        Action::CompleteFilter => {
            if state.overlay == Some(Overlay::Filter)
                && let Some((applied, suggestion)) = super::complete::complete(&state.filter_input)
            {
                state.filter_input = applied;
                state.filter_suggestion = suggestion;
                refresh_filter_parse(state);
            }
            Outcome::Continue
        }
        Action::TextualSubmit => {
            match state.overlay {
                Some(Overlay::Palette) => {
                    state.overlay = None;
                    run_palette_command(state, engine);
                }
                Some(Overlay::Filter) => {
                    state.overlay = None;
                    state.filter_suggestion = None;
                    apply_filter(state);
                }
                Some(Overlay::Edit(_)) => {
                    state.overlay = None;
                    submit_edit(state);
                }
                _ => {}
            }
            Outcome::Continue
        }
    }
}

// Live validity: every keystroke in the filter overlay re-parses the
// expression so the view can show [ok]/[err] with a caret. The applied
// filter itself only changes on submit.
fn refresh_filter_parse(state: &mut AppState) {
    if state.overlay != Some(Overlay::Filter) {
        return;
    }

    if state.filter_input.trim().is_empty() {
        state.filter_error = None;
        return;
    }

    state.filter_error = wntui_core::filter::parse_detailed(&state.filter_input).err();
}

fn is_admin(engine: &EngineState) -> bool {
    engine
        .session
        .as_ref()
        .is_some_and(|session| session.role == UserRole::Admin)
}

fn edit_draft_for(node: &wntui_core::model::Node, key: NodeKey, field: EditField) -> EditDraft {
    let original = match field {
        EditField::Name => node.name.clone().unwrap_or_default(),
        EditField::Description => node.description.clone().unwrap_or_default(),
    };

    EditDraft {
        key,
        field,
        input: original.clone(),
        original,
    }
}

fn open_edit(state: &mut AppState, engine: &EngineState, field: EditField) {
    if !is_admin(engine) {
        // metadata.md: 1061 is admin-only; operators get a hint instead.
        state.message = Some("editing requires an admin session".to_owned());
        return;
    }

    let Some(key) = state.selected else {
        state.message = Some("no node selected".to_owned());
        return;
    };

    let Some(node) = engine.store.node(&key) else {
        state.message = Some("this node is no longer in the live state".to_owned());
        return;
    };

    state.edit = Some(edit_draft_for(node, key, field));
    state.overlay = Some(Overlay::Edit(field));
    state.message = None;
}

// Tab switches the edited field and reseeds the input with that field's
// current live value.
fn rotate_edit_draft(state: &mut AppState, engine: &EngineState, next: EditField) {
    let key = state
        .edit
        .as_ref()
        .map_or(state.selected, |edit| Some(edit.key));

    let Some(key) = key else {
        state.overlay = None;
        return;
    };

    match engine.store.node(&key) {
        Some(node) => {
            state.edit = Some(edit_draft_for(node, key, next));
            state.overlay = Some(Overlay::Edit(next));
        }
        None => {
            state.message = Some("this node is no longer in the live state".to_owned());
            state.overlay = None;
            state.edit = None;
        }
    }
}

fn submit_edit(state: &mut AppState) {
    let Some(edit) = state.edit.take() else {
        return;
    };

    let trimmed = edit.input.trim().to_owned();

    if trimmed == edit.original.trim() {
        state.message = Some(format!("{} unchanged", edit.field.label()));
        return;
    }

    let patch = match edit.field {
        EditField::Name => NodeMetadataPatch::Name(trimmed),
        EditField::Description => NodeMetadataPatch::Description(trimmed),
    };

    let update = NodeMetadataUpdate::new(edit.key.network_id, edit.key.address).with(patch);
    state.pending_edit = Some(update);
    state.message = Some(format!("sending {}…", edit.field.label()));
}

// Opens the graphs page seeded with a voltage fetch for the selected node.
// The fetch is emitted as pending_graph; the runtime forwards it to the
// engine after the reducer returns.
fn open_graphs(state: &mut AppState, engine: &EngineState) {
    let Some(key) = state
        .selected
        .filter(|key| engine.store.node(key).is_some())
    else {
        state.message = Some("no node selected".to_owned());
        return;
    };

    state.page = Page::Graphs;
    start_graph_fetch(
        state,
        key,
        GraphMetric::Voltage,
        default_graph_range(),
        None,
    );
}

fn cycle_graph_range(state: &mut AppState, _engine: &EngineState) {
    let Some(graph) = state.graph.as_ref() else {
        return;
    };

    let range = graph.range.next();
    start_graph_fetch(state, graph.key, graph.metric, range, None);
}

pub fn default_graph_range() -> wntui_core::wire::influx::GraphRange {
    wntui_core::wire::influx::GraphRange::Hour24
}

// Starts a fetch: range-derived (window None) or an absolute window. The
// inflight state is replaced wholesale, so a reply keyed to an older
// (metric, range/window) is dropped when it arrives.
pub fn start_graph_fetch(
    state: &mut AppState,
    key: NodeKey,
    metric: GraphMetric,
    range: wntui_core::wire::influx::GraphRange,
    window: Option<(i64, i64)>,
) {
    let mut request = GraphRequest::new(key, metric, range);
    request.requested = window;
    request.window = window.map(|(start, end)| (start as f64, end as f64));

    // The fetched bounds decide whether a pan/zoom refetches. For a range
    // fetch they are [now - span, now] at request time; for a window
    // fetch, the window itself.
    let now = state.now_ms as f64 / 1000.0;
    request.fetched = match window {
        Some((start, end)) => (start as f64, end as f64),
        None => (now - range.seconds() as f64, now),
    };

    state.graph = Some(request);
    state.pending_graph = Some(PendingGraph {
        key,
        metric,
        range,
        window,
    });
    state.message = Some(match window {
        Some((start, end)) => format!(
            "fetching {} {}…",
            metric.label(),
            super::graphs::window_span_label(start, end)
        ),
        None => format!("fetching {} ({})…", metric.label(), range.label()),
    });
}

// Pan/zoom/jump (N4): pure window math lives in graphs.rs; these decide
// the refetch. A move that stays inside the fetched bounds just moves
// the display window — the zoom fast path.
fn pan_graph(state: &mut AppState, direction: f64) {
    let Some(graph) = state.graph.as_ref() else {
        return;
    };
    if graph.loading {
        return;
    }

    let current = display_window(graph);
    let (window, leaves) = super::graphs::pan_window(current, graph.fetched, direction);

    if leaves {
        start_graph_fetch(
            state,
            graph.key,
            graph.metric,
            graph.range,
            window_i64(window),
        );
    } else if let Some(graph) = state.graph.as_mut() {
        graph.window = Some(window);
    }
}

fn zoom_graph(state: &mut AppState, factor: f64) {
    let Some(graph) = state.graph.as_ref() else {
        return;
    };
    if graph.loading {
        return;
    }

    let current = display_window(graph);
    let (window, leaves) = super::graphs::zoom_window(current, graph.fetched, factor);

    if leaves {
        start_graph_fetch(
            state,
            graph.key,
            graph.metric,
            graph.range,
            window_i64(window),
        );
    } else if let Some(graph) = state.graph.as_mut() {
        graph.window = Some(window);
    }
}

fn jump_graph(state: &mut AppState, to_start: bool) {
    let Some(graph) = state.graph.as_ref() else {
        return;
    };
    if graph.loading {
        return;
    }

    let current = display_window(graph);
    let window = super::graphs::jump_window(current, graph.fetched, to_start);

    // The jump stays inside the fetched bounds by construction; only a
    // degenerate fetched span could leave, and then a refetch is honest.
    let (_, leaves) = super::graphs::pan_window(window, graph.fetched, 0.0);
    if leaves {
        start_graph_fetch(
            state,
            graph.key,
            graph.metric,
            graph.range,
            window_i64(window),
        );
    } else if let Some(graph) = state.graph.as_mut() {
        graph.window = Some(window);
    }
}

fn refetch_graph(state: &mut AppState) {
    let Some(graph) = state.graph.as_ref() else {
        return;
    };
    if graph.loading {
        return;
    }

    start_graph_fetch(
        state,
        graph.key,
        graph.metric,
        graph.range,
        window_i64(display_window(graph)),
    );
}

// The window currently on screen: the display window when set, else the
// whole fetched span.
fn display_window(graph: &GraphRequest) -> (f64, f64) {
    graph.window.unwrap_or(graph.fetched)
}

fn window_i64(window: (f64, f64)) -> Option<(i64, i64)> {
    Some((window.0 as i64, window.1 as i64))
}

fn move_selection(state: &mut AppState, engine: &EngineState, delta: isize) {
    match state.page {
        Page::Networks => {
            move_picker_row(state, engine, delta);
            return;
        }
        Page::NodeDetail => {
            let count = detail_row_count(state, engine);
            let current = state.detail_cursor.min(count.saturating_sub(1));
            let target = if delta.is_positive() {
                (current + delta.unsigned_abs()).min(count.saturating_sub(1))
            } else {
                current.saturating_sub(delta.unsigned_abs())
            };
            state.detail_cursor = target;
            return;
        }
        Page::Graphs | Page::Nodes => {}
    }

    let keys = state.projection.keys().to_vec();
    let current = selection_index_in(&keys, state.selected);

    let target = if delta.is_positive() {
        (current + delta.unsigned_abs()).min(keys.len().saturating_sub(1))
    } else {
        current.saturating_sub(delta.unsigned_abs())
    };

    state.selected = keys.get(target).copied();
    let _ = engine;
}

fn move_picker_row(state: &mut AppState, engine: &EngineState, delta: isize) {
    let entry_count = network_matches(engine, &state.net_query).len();

    let target = if delta.is_positive() {
        (state.picker_row + delta.unsigned_abs()).min(entry_count.saturating_sub(1))
    } else {
        state.picker_row.saturating_sub(delta.unsigned_abs())
    };

    state.picker_row = target;
}

fn detail_row_count(state: &AppState, engine: &EngineState) -> usize {
    state
        .selected
        .and_then(|key| engine.store.node(&key))
        .map(|node| detail_rows(node, engine, state.now_ms).len())
        .unwrap_or(0)
}

fn detail_field_at(state: &AppState, engine: &EngineState) -> Option<DetailField> {
    let node = state.selected.and_then(|key| engine.store.node(&key))?;
    let rows = detail_rows(node, engine, state.now_ms);
    rows.get(state.detail_cursor).map(|row| row.field)
}

// The field under the cursor decides what enter does (review F6):
// editable rows open the editor, graphable rows open their graph.
fn activate_detail_field(state: &mut AppState, engine: &EngineState) {
    match detail_field_at(state, engine) {
        Some(DetailField::Editable(kind)) => {
            let field = match kind {
                EditKind::Name => EditField::Name,
                EditKind::Description => EditField::Description,
            };
            open_edit(state, engine, field);
        }
        Some(DetailField::Graphable(metric)) => {
            let Some(key) = state.selected else {
                return;
            };
            state.page = Page::Graphs;
            state.detail_cursor = 0;
            start_graph_fetch(state, key, metric, default_graph_range(), None);
        }
        _ => {}
    }
}

// The networks page is a searchable dropdown (review F7): the query
// filters by name substring (case-insensitive) or id prefix; "all"
// keeps the all-networks entry selectable.
pub fn network_matches(engine: &EngineState, query: &str) -> Vec<Option<u32>> {
    let query = query.trim().to_lowercase();

    let mut entries = Vec::new();
    if query.is_empty() || "all networks".contains(&query) {
        entries.push(None);
    }

    engine
        .networks
        .iter()
        .filter(|network| {
            network.name.to_lowercase().contains(&query)
                || network.id.to_string().starts_with(&query)
        })
        .for_each(|network| entries.push(Some(network.id)));

    entries
}

fn select_highlighted_network(state: &mut AppState, engine: &EngineState) {
    let entries = network_matches(engine, &state.net_query);

    match entries.get(state.picker_row.min(entries.len().saturating_sub(1))) {
        Some(entry) => {
            state.selected_network = *entry;
            state.page = Page::Nodes;
            state.selected = None;
            state.net_query.clear();
            state.picker_row = 0;
        }
        None => state.message = Some("no networks discovered yet".to_owned()),
    }
}

fn apply_filter(state: &mut AppState) {
    if state.filter_input.trim().is_empty() {
        state.filter = None;
        state.filter_text.clear();
        state.message = None;
        return;
    }

    match wntui_core::filter::parse(&state.filter_input) {
        Ok(filter) => {
            state.filter = Some(filter);
            state.filter_text = state.filter_input.trim().to_owned();
            state.message = None;
        }
        Err(error) => {
            state.message = Some(error.to_string());
        }
    }
}

fn run_palette_command(state: &mut AppState, engine: &EngineState) {
    state.message = None;

    match parse_command(&state.palette_input) {
        super::palette::PaletteCommand::Quit => state.quit = true,
        super::palette::PaletteCommand::Help => state.overlay = Some(Overlay::Help),
        super::palette::PaletteCommand::Fields => state.overlay = Some(Overlay::Fields),
        super::palette::PaletteCommand::NetworksPicker => {
            state.page = Page::Networks;
            state.picker_row = 0;
        }
        super::palette::PaletteCommand::SelectNetwork { query } => {
            match resolve_network(&engine.networks, &query) {
                Some(network) => {
                    state.selected_network = Some(network.id);
                    state.page = Page::Nodes;
                    state.selected = None;
                }
                None => state.message = Some(format!("no network matches '{query}'")),
            }
        }
        super::palette::PaletteCommand::Edit => {
            open_edit(state, engine, EditField::Name);
        }
        super::palette::PaletteCommand::Graph { metric } => {
            let metric = match metric.as_deref().map(GraphMetric::parse) {
                Some(Some(metric)) => metric,
                Some(None) => {
                    state.message = Some(format!(
                        "unknown metric '{}' — voltage, traveltime, traveltime-qos1, online, boots, packets",
                        metric.unwrap_or_default()
                    ));
                    return;
                }
                None => GraphMetric::Voltage,
            };

            if let Some(key) = state
                .selected
                .filter(|key| engine.store.node(key).is_some())
            {
                state.page = Page::Graphs;
                start_graph_fetch(state, key, metric, default_graph_range(), None);
            } else {
                state.message = Some("no node selected".to_owned());
            }
        }
        super::palette::PaletteCommand::Sort { query } => {
            // `:sort` cycles, `:sort <field>` picks the field, and a
            // trailing `asc`/`desc` sets the direction in one shot.
            let Some(query) = query else {
                state.sort = state.sort.next();
                state.message = Some(format!("sorted by {}", state.sort.label()));
                return;
            };

            let mut words = query.split_whitespace();
            let Some(field_word) = words.next() else {
                state.sort = state.sort.next();
                state.message = Some(format!("sorted by {}", state.sort.label()));
                return;
            };

            let Some(field) = SortField::parse(field_word) else {
                state.message = Some(format!(
                    "unknown sort field '{field_word}' — addr, name, status, role, voltage, fw, age"
                ));
                return;
            };

            state.sort = field;
            state.sort_reverse = match words.next() {
                Some("desc") | Some("descending") => true,
                Some("asc") | Some("ascending") => false,
                None => state.sort_reverse,
                Some(other) => {
                    state.message = Some(format!("unknown direction '{other}' — use asc or desc"));
                    return;
                }
            };

            let direction = if state.sort_reverse {
                "descending"
            } else {
                "ascending"
            };
            state.message = Some(format!("sorted by {} ({direction})", state.sort.label()));
        }
        super::palette::PaletteCommand::FilterSave { name } => {
            let Some(name) = name else {
                state.message =
                    Some("usage: :filter-save <name> — saves the current filter".to_owned());
                return;
            };

            if state.filter_text.is_empty() {
                state.message = Some("no filter is active — set one with / first".to_owned());
                return;
            }

            state
                .saved_filters
                .insert(name.clone(), state.filter_text.clone());
            state.pending_filter_save = Some((name.clone(), state.filter_text.clone()));
            state.message = Some(format!("filter saved as '{name}'"));
        }
        super::palette::PaletteCommand::FilterLoad { name } => {
            let Some(name) = name else {
                state.message = Some("usage: :filter-load <name>".to_owned());
                return;
            };

            match state.saved_filters.get(&name) {
                Some(expression) => {
                    state.filter_text = expression.clone();
                    state.filter = wntui_core::filter::parse(expression).ok();
                    state.message = Some(format!("filter '{name}' loaded"));
                }
                None => {
                    state.message = Some(format!("no saved filter named '{name}'"));
                }
            }
        }
        super::palette::PaletteCommand::FiltersList => {
            if state.saved_filters.is_empty() {
                state.message = Some("no saved filters yet — :filter-save <name>".to_owned());
                return;
            }

            let list = state
                .saved_filters
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            state.message = Some(format!("saved filters: {list}"));
        }
        super::palette::PaletteCommand::Unknown(head) => {
            state.message = Some(format!("unknown command ':{head}'"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::listing::NodeView;
    use pretty_assertions::assert_eq;
    use wntui_core::model::{Network, NodeKey};

    fn demo_engine() -> EngineState {
        let mut engine = EngineState::default();
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));
        engine.networks = vec![Network {
            id: 42,
            name: "Demo plant".to_owned(),
        }];
        engine
    }

    fn addresses(engine: &EngineState, state: &AppState) -> Vec<u32> {
        NodeView::from_app(state)
            .nodes(engine)
            .iter()
            .map(|node| node.key.address)
            .collect()
    }

    #[test]
    fn movement_walks_the_sorted_list_by_key() {
        let engine = demo_engine();
        let mut state = AppState::new();

        update(&mut state, Action::ToTop, &engine);
        assert_eq!(state.selected, Some(NodeKey::new(42, 1)));

        update(&mut state, Action::MoveDown, &engine);
        update(&mut state, Action::MoveDown, &engine);
        assert_eq!(state.selected, Some(NodeKey::new(42, 3)));

        update(&mut state, Action::MoveUp, &engine);
        assert_eq!(state.selected, Some(NodeKey::new(42, 2)));

        update(&mut state, Action::ToBottom, &engine);
        assert_eq!(state.selected, Some(NodeKey::new(42, 8)));

        update(&mut state, Action::MoveDown, &engine);
        assert_eq!(state.selected, Some(NodeKey::new(42, 8)));
    }

    #[test]
    fn selection_survives_sort_flips() {
        let engine = demo_engine();
        let mut state = AppState::new();

        update(&mut state, Action::ToBottom, &engine);
        let last_by_address = state.selected;

        update(&mut state, Action::ReverseSort, &engine);
        assert_eq!(state.selected, last_by_address);

        let order = addresses(&engine, &state);
        assert_eq!(
            order.first().copied(),
            last_by_address.map(|key| key.address)
        );
    }

    #[test]
    fn half_and_full_page_move_by_the_page_height() {
        let engine = demo_engine();
        let mut state = AppState::new();
        state.page_height = 4;

        update(&mut state, Action::ToTop, &engine);
        update(&mut state, Action::HalfPageDown, &engine);
        assert_eq!(state.selected, Some(NodeKey::new(42, 3)));

        update(&mut state, Action::FullPageUp, &engine);
        assert_eq!(state.selected, Some(NodeKey::new(42, 1)));
    }

    #[test]
    fn sort_cycling_and_reversing_leave_a_message() {
        let engine = demo_engine();
        let mut state = AppState::new();

        update(&mut state, Action::CycleSort, &engine);
        assert_eq!(state.sort, crate::tui::listing::SortField::Name);
        assert_eq!(state.message.as_deref(), Some("sorted by name"));

        update(&mut state, Action::ReverseSort, &engine);
        assert!(state.sort_reverse);
        assert_eq!(
            state.message.as_deref(),
            Some("sorted by name (descending)")
        );
    }

    #[test]
    fn filter_submit_applies_clears_and_reports_errors() {
        let engine = demo_engine();
        let mut state = AppState::new();

        state.overlay = Some(Overlay::Filter);
        state.filter_input = "status:offline".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert!(state.filter.is_some());
        assert_eq!(state.filter_text, "status:offline");
        assert_eq!(
            addresses(&engine, &state),
            vec![5, 8],
            "two offline nodes in the demo network"
        );

        state.overlay = Some(Overlay::Filter);
        state.filter_input = "voltge:<3".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert!(
            state
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("unknown field")
        );
        assert!(
            state.filter_text == "status:offline",
            "previous filter stays active"
        );

        state.overlay = Some(Overlay::Filter);
        state.filter_input = "  ".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert!(state.filter.is_none());
        assert_eq!(addresses(&engine, &state).len(), 8);
    }

    #[test]
    fn filter_error_keeps_previous_filter_active() {
        let engine = demo_engine();
        let mut state = AppState::new();

        state.overlay = Some(Overlay::Filter);
        state.filter_input = "role:sink".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert_eq!(addresses(&engine, &state), vec![1]);

        state.overlay = Some(Overlay::Filter);
        state.filter_input = "role:banana".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert!(state.message.is_some());
        assert_eq!(addresses(&engine, &state), vec![1]);
    }

    #[test]
    fn filter_typing_updates_the_live_error_state() {
        let engine = demo_engine();
        let mut state = AppState::new();

        update(&mut state, Action::OpenFilter, &engine);
        assert!(state.filter_error.is_none(), "empty input is not an error");

        for character in "role:banana".chars() {
            update(&mut state, Action::TextualInput(character), &engine);
        }
        let error = state.filter_error.clone().expect("live error while typing");
        assert!(error.to_string().contains("bad value"));
        assert_eq!(error.position, 5, "the caret points at the value");

        // A dangling operator is incomplete rather than wrong: Wireshark
        // also marks mid-typed expressions invalid until they finish.
        for _ in 0..6 {
            update(&mut state, Action::TextualBackspace, &engine);
        }
        assert_eq!(state.filter_input, "role:");
        assert!(state.filter_error.is_some());
        assert!(
            state
                .filter_error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("missing value"),
            "the error names what is missing"
        );

        for character in "sink".chars() {
            update(&mut state, Action::TextualInput(character), &engine);
        }
        assert_eq!(state.filter_error, None, "finished expression is valid");
    }

    #[test]
    fn tab_completes_fields_and_values_in_the_filter_overlay() {
        let engine = demo_engine();
        let mut state = AppState::new();

        update(&mut state, Action::OpenFilter, &engine);
        for character in "vol".chars() {
            update(&mut state, Action::TextualInput(character), &engine);
        }
        update(&mut state, Action::CompleteFilter, &engine);
        assert_eq!(state.filter_input, "voltage ");
        assert!(state.filter_suggestion.is_some());

        // A fully typed field is a word candidate no more: Tab now inserts
        // the first operator, attached without a gap.
        update(&mut state, Action::CompleteFilter, &engine);
        assert_eq!(state.filter_input, "voltage:");

        for character in "3.".chars() {
            update(&mut state, Action::TextualInput(character), &engine);
        }
        update(&mut state, Action::CompleteFilter, &engine);
        // voltage has no enum values: nothing to complete, input unchanged.
        assert_eq!(state.filter_input, "voltage:3.");
    }

    #[test]
    fn tab_completes_enum_values_after_a_colon() {
        let engine = demo_engine();
        let mut state = AppState::new();

        update(&mut state, Action::OpenFilter, &engine);
        for character in "status:o".chars() {
            update(&mut state, Action::TextualInput(character), &engine);
        }
        update(&mut state, Action::CompleteFilter, &engine);
        assert_eq!(state.filter_input, "status:online ");
        assert_eq!(
            state.filter_suggestion.as_deref(),
            Some("online offline uncertain"),
            "the remaining choices stay visible"
        );
        assert_eq!(state.filter_error, None);
    }

    #[test]
    fn filter_save_and_load_round_trip_through_the_palette() {
        let engine = demo_engine();
        let mut state = AppState::new();

        state.overlay = Some(Overlay::Filter);
        state.filter_input = "status:offline".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert_eq!(state.filter_text, "status:offline");

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "filter-save offline-set".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert_eq!(
            state.saved_filters.get("offline-set"),
            Some(&"status:offline".to_owned())
        );
        assert_eq!(
            state.pending_filter_save,
            Some(("offline-set".to_owned(), "status:offline".to_owned()))
        );

        // Overwrite the live filter, then load the profile back.
        state.overlay = Some(Overlay::Filter);
        state.filter_input = "role:sink".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert_eq!(state.filter_text, "role:sink");

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "filter-load offline-set".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert_eq!(state.filter_text, "status:offline");
        assert!(state.filter.is_some());
        assert_eq!(addresses(&engine, &state), vec![5, 8]);

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "filter-load nope".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert!(
            state
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("no saved filter")
        );
    }

    #[test]
    fn fields_command_opens_the_reference_overlay() {
        let mut state = AppState::new();

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "fields".to_owned();
        update(&mut state, Action::TextualSubmit, &EngineState::default());
        assert_eq!(state.overlay, Some(Overlay::Fields));

        update(&mut state, Action::Back, &EngineState::default());
        assert_eq!(state.overlay, None);
    }

    #[test]
    fn back_from_help_restores_the_page() {
        let mut state = AppState::new();
        state.overlay = Some(Overlay::Help);
        update(&mut state, Action::Back, &EngineState::default());
        assert_eq!(state.overlay, None);
        assert_eq!(state.page, Page::Nodes);
    }
}

#[cfg(test)]
mod palette_regressions {
    use super::super::app::{AppState, Overlay, Page};
    use super::super::input::Action;
    use super::*;
    use pretty_assertions::assert_eq;
    use wntui_core::model::Network;

    #[test]
    fn palette_quit_sets_quit() {
        let mut state = AppState::new();
        state.overlay = Some(Overlay::Palette);
        state.palette_input = "q".to_owned();
        update(&mut state, Action::TextualSubmit, &EngineState::default());
        assert!(state.quit);
    }

    #[test]
    fn palette_net_opens_the_picker_and_net_query_selects() {
        let mut engine = EngineState::default();
        engine.networks = vec![Network {
            id: 42,
            name: "Plant floor".to_owned(),
        }];

        let mut state = AppState::new();
        state.overlay = Some(Overlay::Palette);
        state.palette_input = "net".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert_eq!(state.page, Page::Networks);

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "net plant".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);
        assert_eq!(state.selected_network, Some(42));
        assert_eq!(state.page, Page::Nodes);
    }

    #[test]
    fn unknown_palette_command_leaves_a_message() {
        let mut state = AppState::new();
        state.overlay = Some(Overlay::Palette);
        state.palette_input = "frob".to_owned();
        update(&mut state, Action::TextualSubmit, &EngineState::default());
        assert_eq!(state.message.as_deref(), Some("unknown command ':frob'"));
    }

    #[test]
    fn palette_sort_cycles_picks_fields_and_sets_direction() {
        let mut state = AppState::new();

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "sort".to_owned();
        update(&mut state, Action::TextualSubmit, &EngineState::default());
        assert_eq!(state.sort, SortField::Name);
        assert_eq!(state.message.as_deref(), Some("sorted by name"));

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "sort status desc".to_owned();
        update(&mut state, Action::TextualSubmit, &EngineState::default());
        assert_eq!(state.sort, SortField::Status);
        assert!(state.sort_reverse);
        assert_eq!(
            state.message.as_deref(),
            Some("sorted by status (descending)")
        );

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "sort role asc".to_owned();
        update(&mut state, Action::TextualSubmit, &EngineState::default());
        assert_eq!(state.sort, SortField::Role);
        assert!(!state.sort_reverse);

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "sort banana".to_owned();
        update(&mut state, Action::TextualSubmit, &EngineState::default());
        assert_eq!(state.sort, SortField::Role, "the previous sort stays");
        assert!(
            state
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("unknown sort field")
        );

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "sort name sideways".to_owned();
        update(&mut state, Action::TextualSubmit, &EngineState::default());
        assert!(
            state
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("unknown direction")
        );
    }

    #[test]
    fn filter_overlay_routes_text_input() {
        let mut state = AppState::new();
        state.overlay = Some(Overlay::Filter);

        update(
            &mut state,
            Action::TextualInput('s'),
            &EngineState::default(),
        );
        update(
            &mut state,
            Action::TextualInput('i'),
            &EngineState::default(),
        );
        update(
            &mut state,
            Action::TextualBackspace,
            &EngineState::default(),
        );
        assert_eq!(state.filter_input, "s");
        assert_eq!(state.overlay, Some(Overlay::Filter));
    }
}

#[cfg(test)]
mod detail_navigation {
    use super::*;
    use pretty_assertions::assert_eq;
    use wntui_core::model::NodeKey;

    fn engine() -> EngineState {
        let mut engine = EngineState::default();
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));
        engine
    }

    #[test]
    fn enter_opens_the_detail_of_the_selected_node() {
        let engine = engine();
        let mut state = AppState::new();

        update(&mut state, Action::ToTop, &engine);
        update(&mut state, Action::MoveDown, &engine);
        update(&mut state, Action::Enter, &engine);

        assert_eq!(state.page, Page::NodeDetail);
        assert_eq!(state.selected, Some(NodeKey::new(42, 2)));
        assert_eq!(state.detail_cursor, 0);
    }

    #[test]
    fn enter_without_nodes_stays_on_the_list() {
        let mut state = AppState::new();

        update(&mut state, Action::Enter, &EngineState::default());
        assert_eq!(state.page, Page::Nodes);
    }

    #[test]
    fn enter_with_no_prior_selection_opens_the_first_node() {
        let engine = engine();
        let mut state = AppState::new();

        update(&mut state, Action::Enter, &engine);
        assert_eq!(state.page, Page::NodeDetail);
        assert_eq!(state.selected, Some(NodeKey::new(42, 1)));
    }

    #[test]
    fn back_from_detail_returns_to_the_list() {
        let engine = engine();
        let mut state = AppState::new();

        update(&mut state, Action::ToTop, &engine);
        update(&mut state, Action::Enter, &engine);
        update(&mut state, Action::Back, &engine);

        assert_eq!(state.page, Page::Nodes);
        assert_eq!(state.selected, Some(NodeKey::new(42, 1)));
    }

    #[test]
    fn detail_cursor_moves_line_by_line_and_clamps() {
        let engine = engine();
        let mut state = AppState::new();

        update(&mut state, Action::ToTop, &engine);
        update(&mut state, Action::Enter, &engine);
        assert_eq!(state.detail_cursor, 0);

        update(&mut state, Action::MoveDown, &engine);
        update(&mut state, Action::MoveDown, &engine);
        assert_eq!(state.detail_cursor, 2);

        update(&mut state, Action::MoveUp, &engine);
        assert_eq!(state.detail_cursor, 1);

        update(&mut state, Action::ToTop, &engine);
        assert_eq!(state.detail_cursor, 0);

        update(&mut state, Action::ToBottom, &engine);
        let last = state.detail_cursor;
        assert!(last >= 30, "sink detail has 30+ rows, got {last}");

        update(&mut state, Action::MoveDown, &engine);
        assert_eq!(state.detail_cursor, last, "cursor clamps to the last row");

        update(&mut state, Action::MoveUp, &engine);
        assert_eq!(state.detail_cursor, last.saturating_sub(1));
    }

    fn cursor_to(state: &mut AppState, engine: &EngineState, field: DetailField) {
        let key = state.selected.expect("a node is selected");
        let node = engine.store.node(&key).expect("node is in the live state");
        let rows = detail_rows(node, engine, state.now_ms);
        let index = rows
            .iter()
            .position(|row| row.field == field)
            .expect("field is present in the projection");
        state.detail_cursor = index;
    }

    #[test]
    fn enter_on_an_editable_field_opens_the_editor() {
        let mut engine = engine();
        engine.session = Some(wntui_core::session::SessionInfo {
            session_id: "s".to_owned(),
            role: UserRole::Admin,
            influx_token: None,
        });
        let mut state = AppState::new();

        update(&mut state, Action::ToTop, &engine);
        update(&mut state, Action::Enter, &engine);
        cursor_to(&mut state, &engine, DetailField::Editable(EditKind::Name));

        update(&mut state, Action::Enter, &engine);

        assert_eq!(state.overlay, Some(Overlay::Edit(EditField::Name)));
        let edit = state.edit.expect("edit draft opened");
        assert_eq!(edit.input, "Sink A");
    }

    #[test]
    fn enter_on_a_graphable_field_fetches_that_graph() {
        let engine = engine();
        let mut state = AppState::new();

        update(&mut state, Action::ToTop, &engine);
        update(&mut state, Action::Enter, &engine);
        cursor_to(
            &mut state,
            &engine,
            DetailField::Graphable(GraphMetric::Voltage),
        );

        update(&mut state, Action::Enter, &engine);

        assert_eq!(state.page, Page::Graphs);
        let graph = state.graph.expect("graph fetch started");
        assert_eq!(graph.metric, GraphMetric::Voltage);
        assert_eq!(graph.key, NodeKey::new(42, 1));
    }

    #[test]
    fn enter_on_a_plain_field_does_nothing() {
        let engine = engine();
        let mut state = AppState::new();

        update(&mut state, Action::ToTop, &engine);
        update(&mut state, Action::Enter, &engine);
        update(&mut state, Action::MoveDown, &engine);
        update(&mut state, Action::MoveDown, &engine);
        let on_section_header = state.detail_cursor;

        update(&mut state, Action::Enter, &engine);

        assert_eq!(state.page, Page::NodeDetail);
        assert_eq!(state.overlay, None);
        assert!(state.graph.is_none());
        assert_eq!(state.detail_cursor, on_section_header);
    }
}

#[cfg(test)]
mod network_picker {
    use super::super::app::{AppState, Page};
    use super::*;
    use pretty_assertions::assert_eq;
    use wntui_core::model::Network;

    fn engine() -> EngineState {
        let mut engine = EngineState::default();
        engine.networks = vec![
            Network {
                id: 42,
                name: "Demo plant".to_owned(),
            },
            Network {
                id: 7,
                name: "Warehouse".to_owned(),
            },
        ];
        engine
    }

    fn picker() -> AppState {
        AppState {
            page: Page::Networks,
            ..AppState::new()
        }
    }

    #[test]
    fn query_filters_by_name_substring_and_id_prefix() {
        let engine = engine();

        assert_eq!(
            network_matches(&engine, ""),
            vec![None, Some(42), Some(7)],
            "empty query lists everything, all-networks first"
        );
        assert_eq!(network_matches(&engine, "demo"), vec![Some(42)]);
        assert_eq!(network_matches(&engine, "WARE"), vec![Some(7)]);
        assert_eq!(network_matches(&engine, "4"), vec![Some(42)]);
        assert_eq!(network_matches(&engine, "all"), vec![None]);
        assert!(network_matches(&engine, "zzz").is_empty());
    }

    #[test]
    fn typing_feeds_the_query_and_resets_the_highlight() {
        let engine = engine();
        let mut state = picker();
        state.picker_row = 1;

        update(&mut state, Action::TextualInput('w'), &engine);
        update(&mut state, Action::TextualInput('a'), &engine);
        assert_eq!(state.net_query, "wa");
        assert_eq!(state.picker_row, 0);

        update(&mut state, Action::TextualBackspace, &engine);
        update(&mut state, Action::TextualBackspace, &engine);
        assert_eq!(state.net_query, "");
    }

    #[test]
    fn movement_clamps_to_the_filtered_count() {
        let engine = engine();
        let mut state = picker();

        update(&mut state, Action::MoveDown, &engine);
        update(&mut state, Action::MoveDown, &engine);
        assert_eq!(state.picker_row, 2, "three entries: all, 42, 7");

        update(&mut state, Action::MoveDown, &engine);
        assert_eq!(state.picker_row, 2, "clamped to the last entry");

        update(&mut state, Action::TextualInput('h'), &engine);
        update(&mut state, Action::TextualInput('o'), &engine);
        update(&mut state, Action::MoveDown, &engine);
        assert_eq!(state.picker_row, 0, "one match stays highlighted at row 0");

        update(&mut state, Action::MoveUp, &engine);
        assert_eq!(state.picker_row, 0, "moving up clamps at the top");
    }

    #[test]
    fn to_top_and_to_bottom_move_the_picker_row() {
        let engine = engine();
        let mut state = picker();

        update(&mut state, Action::ToBottom, &engine);
        assert_eq!(state.picker_row, 2);
        assert!(state.selected.is_none(), "the node selection is untouched");

        update(&mut state, Action::ToTop, &engine);
        assert_eq!(state.picker_row, 0);
    }

    #[test]
    fn enter_selects_the_filtered_entry_and_resets_the_query() {
        let engine = engine();
        let mut state = picker();

        update(&mut state, Action::TextualInput('h'), &engine);
        update(&mut state, Action::TextualInput('o'), &engine);
        update(&mut state, Action::Enter, &engine);

        assert_eq!(state.selected_network, Some(7));
        assert_eq!(state.page, Page::Nodes);
        assert_eq!(state.net_query, "");
        assert_eq!(state.picker_row, 0);
    }

    #[test]
    fn enter_with_no_matches_leaves_a_message() {
        let engine = engine();
        let mut state = picker();

        update(&mut state, Action::TextualInput('z'), &engine);
        update(&mut state, Action::Enter, &engine);

        assert_eq!(state.page, Page::Networks);
        assert_eq!(state.message.as_deref(), Some("no networks discovered yet"));
    }
}

#[cfg(test)]
mod graph_flows {
    use super::super::app::{AppState, Page};
    use super::*;
    use pretty_assertions::assert_eq;
    use wntui_core::model::NodeKey;
    use wntui_core::wire::influx::{GraphMetric, GraphRange, Series, SeriesPoint};

    fn engine() -> EngineState {
        let mut engine = EngineState::default();
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));
        engine
    }

    fn detail_state(address: u32) -> AppState {
        AppState {
            page: Page::NodeDetail,
            selected: Some(NodeKey::new(42, address)),
            ..AppState::new()
        }
    }

    #[test]
    fn open_graphs_emits_a_voltage_fetch_for_the_selected_node() {
        let engine = engine();
        let mut state = detail_state(4);

        update(&mut state, Action::OpenGraphs, &engine);

        assert_eq!(state.page, Page::Graphs);
        let pending = state.pending_graph.expect("a fetch was emitted");
        assert_eq!(pending.key, NodeKey::new(42, 4));
        assert_eq!(pending.metric, GraphMetric::Voltage);
        assert_eq!(pending.range, GraphRange::Hour24);
        assert_eq!(pending.window, None);

        let graph = state.graph.as_ref().expect("inflight graph state");
        assert!(graph.loading);
    }

    #[test]
    fn open_graphs_without_a_node_hints_instead() {
        let engine = engine();
        let mut state = AppState::new();

        update(&mut state, Action::OpenGraphs, &engine);

        assert_eq!(state.page, Page::Nodes);
        assert_eq!(state.message.as_deref(), Some("no node selected"));
        assert!(state.pending_graph.is_none());
    }

    #[test]
    fn range_cycling_emits_a_fetch_per_step_and_replaces_the_request() {
        let engine = engine();
        let mut state = detail_state(2);

        update(&mut state, Action::OpenGraphs, &engine);
        // open seeds 24h, so cycling walks 7d -> 1h -> 6h -> 24h
        for expected in [
            GraphRange::Day7,
            GraphRange::Hour1,
            GraphRange::Hour6,
            GraphRange::Hour24,
        ] {
            update(&mut state, Action::CycleGraphRange, &engine);
            let pending = state.pending_graph.expect("fetch emitted");
            assert_eq!(pending.range, expected);
            assert_eq!(state.graph.as_ref().expect("graph").range, expected);
            assert!(state.graph.as_ref().expect("graph").loading);
        }
    }

    #[test]
    fn range_cycling_without_a_graph_is_a_noop() {
        let mut state = detail_state(2);
        update(&mut state, Action::CycleGraphRange, &EngineState::default());
        assert!(state.pending_graph.is_none());
        assert!(state.graph.is_none());
    }

    #[test]
    fn back_from_graphs_returns_to_the_detail_page() {
        let engine = engine();
        let mut state = detail_state(4);

        update(&mut state, Action::OpenGraphs, &engine);
        update(&mut state, Action::Back, &engine);

        assert_eq!(state.page, Page::NodeDetail);
    }

    #[test]
    fn graph_data_lands_only_on_the_matching_request() {
        let engine = engine();
        let mut state = detail_state(4);
        update(&mut state, Action::OpenGraphs, &engine);

        let series = vec![Series {
            name: "analytics_boot".to_owned(),
            columns: vec!["time".to_owned(), "mean".to_owned()],
            points: vec![SeriesPoint {
                timestamp_ns: 1,
                values: vec![serde_json::json!(3.1)],
            }],
        }];

        // stale range: dropped
        state.apply_engine_event(wntui_core::engine::EngineEvent::GraphData {
            key: NodeKey::new(42, 4),
            metric: GraphMetric::Voltage,
            range: GraphRange::Hour1,
            window: None,
            series: series.clone(),
        });
        assert!(state.graph.as_ref().expect("graph").loading);

        // matching range: absorbed
        state.apply_engine_event(wntui_core::engine::EngineEvent::GraphData {
            key: NodeKey::new(42, 4),
            metric: GraphMetric::Voltage,
            range: GraphRange::Hour24,
            window: None,
            series,
        });
        let graph = state.graph.as_ref().expect("graph");
        assert!(!graph.loading);
        assert_eq!(graph.series.len(), 1);

        // failure for a stale range is dropped too
        state.apply_engine_event(wntui_core::engine::EngineEvent::GraphFailed {
            key: NodeKey::new(42, 4),
            metric: GraphMetric::Voltage,
            range: GraphRange::Hour1,
            window: None,
            reason: "boom".to_owned(),
        });
        assert!(state.graph.as_ref().expect("graph").error.is_none());
    }

    #[test]
    fn palette_graph_selects_the_metric_for_the_selected_node() {
        let engine = engine();
        let mut state = detail_state(2);

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "graph traveltime".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);

        assert_eq!(state.page, Page::Graphs);
        let pending = state.pending_graph.expect("fetch emitted");
        assert_eq!(pending.metric, GraphMetric::TraveltimeQos0);
    }

    #[test]
    fn palette_graph_with_an_unknown_metric_hints_and_stays() {
        let engine = engine();
        let mut state = detail_state(2);

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "graph banana".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);

        assert_eq!(state.page, Page::NodeDetail);
        assert!(state.pending_graph.is_none());
        assert!(
            state
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("unknown metric")
        );
    }

    // A graph whose fetch landed: 24h of range ending at the fixed now,
    // with two samples on hand.
    fn landed_graph() -> AppState {
        let engine = engine();
        let mut state = detail_state(4);
        state.now_ms = 1_700_000_005_000;

        update(&mut state, Action::OpenGraphs, &engine);
        state.apply_engine_event(wntui_core::engine::EngineEvent::GraphData {
            key: NodeKey::new(42, 4),
            metric: GraphMetric::Voltage,
            range: GraphRange::Hour24,
            window: None,
            series: vec![Series {
                name: "analytics_boot".to_owned(),
                columns: vec!["time".to_owned(), "mean".to_owned()],
                points: vec![
                    SeriesPoint {
                        timestamp_ns: 1_699_913_605_000_000_000,
                        values: vec![serde_json::json!(3.1)],
                    },
                    SeriesPoint {
                        timestamp_ns: 1_700_000_005_000_000_000,
                        values: vec![serde_json::json!(3.3)],
                    },
                ],
            }],
        });
        // The runtime loop drains pending_graph; tests do it here so the
        // assertions below see only what pan/zoom emit.
        state.pending_graph = None;
        state
    }

    #[test]
    fn zoom_in_is_the_fast_path_no_refetch() {
        let engine = engine();
        let mut state = landed_graph();

        update(&mut state, Action::ZoomInGraph, &engine);

        let graph = state.graph.as_ref().expect("graph");
        assert!(state.pending_graph.is_none(), "zooming in never refetches");
        let window = graph.window.expect("the display window is set");
        assert!((window.1 - window.0 - 12.0 * 3600.0).abs() < 1e-6);
        assert!(!graph.loading);
    }

    #[test]
    fn pan_after_a_zoom_stays_inside_the_fetched_series() {
        let engine = engine();
        let mut state = landed_graph();

        update(&mut state, Action::ZoomInGraph, &engine);
        update(&mut state, Action::PanGraphLeft, &engine);

        let graph = state.graph.as_ref().expect("graph");
        assert!(state.pending_graph.is_none());
        let window = graph.window.expect("window moved");
        assert!(window.0 < 1_700_000_005.0 - 6.0 * 3600.0);
    }

    #[test]
    fn pan_past_the_fetched_bounds_refetches_the_new_window() {
        let engine = engine();
        let mut state = landed_graph();

        // Panning from the full fetched view always asks for times the
        // range fetch does not cover: a windowed fetch goes out.
        update(&mut state, Action::PanGraphLeft, &engine);

        let pending = state.pending_graph.expect("a windowed fetch");
        assert_eq!(pending.window, Some((1_699_870_405, 1_699_956_805)));
        let graph = state.graph.as_ref().expect("graph");
        assert_eq!(graph.requested, Some((1_699_870_405, 1_699_956_805)));
        assert!(graph.loading);
    }

    #[test]
    fn zoom_out_beyond_the_fetched_bounds_refetches() {
        let engine = engine();
        let mut state = landed_graph();

        update(&mut state, Action::ZoomOutGraph, &engine);

        let pending = state.pending_graph.expect("a wider window is fetched");
        let (start, end) = pending.window.expect("windowed");
        assert!((end - start - 48 * 3600).abs() < 2);
    }

    #[test]
    fn jumps_pin_to_the_fetched_edges_without_refetching() {
        let engine = engine();
        let mut state = landed_graph();

        update(&mut state, Action::ZoomInGraph, &engine);
        update(&mut state, Action::GraphJumpStart, &engine);
        let graph = state.graph.as_ref().expect("graph");
        let window = graph.window.expect("window");
        assert!((window.0 - (1_700_000_005.0 - 24.0 * 3600.0)).abs() < 1.0);
        assert!(state.pending_graph.is_none());

        update(&mut state, Action::GraphJumpEnd, &engine);
        let window = state.graph.as_ref().expect("graph").window.expect("window");
        assert!((window.1 - 1_700_000_005.0).abs() < 1.0);
        assert!(state.pending_graph.is_none());
    }

    #[test]
    fn refetch_re_fetches_the_display_window() {
        let engine = engine();
        let mut state = landed_graph();

        update(&mut state, Action::RefetchGraph, &engine);

        let pending = state.pending_graph.expect("a refetch");
        // The display window is the whole fetched range: 24h ending now.
        assert_eq!(pending.window, Some((1_699_913_605, 1_700_000_005)));
    }

    #[test]
    fn pan_and_zoom_are_ignored_while_loading() {
        let engine = engine();
        let mut state = detail_state(4);
        update(&mut state, Action::OpenGraphs, &engine);
        state.pending_graph = None;

        update(&mut state, Action::PanGraphLeft, &engine);
        update(&mut state, Action::ZoomInGraph, &engine);
        update(&mut state, Action::RefetchGraph, &engine);

        assert!(state.pending_graph.is_none(), "the initial fetch stands");
        let graph = state.graph.as_ref().expect("graph");
        assert!(graph.window.is_none());
    }

    #[test]
    fn stale_window_replies_are_dropped_by_the_guard() {
        let engine = engine();
        let mut state = landed_graph();

        // A pan beyond the fetched bounds starts a windowed fetch.
        update(&mut state, Action::PanGraphLeft, &engine);
        let requested = (1_699_870_405, 1_699_956_805);

        // A reply for a different window: dropped.
        state.apply_engine_event(wntui_core::engine::EngineEvent::GraphData {
            key: NodeKey::new(42, 4),
            metric: GraphMetric::Voltage,
            range: GraphRange::Hour24,
            window: Some((1, 2)),
            series: vec![],
        });
        assert!(
            state.graph.as_ref().expect("graph").loading,
            "the stale window reply was dropped"
        );

        // The matching window: absorbed, and the display window is the
        // one the operator panned to.
        state.apply_engine_event(wntui_core::engine::EngineEvent::GraphData {
            key: NodeKey::new(42, 4),
            metric: GraphMetric::Voltage,
            range: GraphRange::Hour24,
            window: Some(requested),
            series: vec![],
        });
        let graph = state.graph.as_ref().expect("graph");
        assert!(!graph.loading);
        assert_eq!(graph.requested, Some(requested));
        assert_eq!(graph.window, Some((requested.0 as f64, requested.1 as f64)));
    }
}

#[cfg(test)]
mod edit_flows {
    use super::super::app::{AppState, EditField, Overlay, Page};
    use super::*;
    use pretty_assertions::assert_eq;
    use wntui_core::model::NodeKey;
    use wntui_core::wire::metadata::NodeMetadataPatch;

    fn engine(role: Option<UserRole>) -> EngineState {
        let mut engine = EngineState::default();
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));

        engine.session = role.map(|role| wntui_core::session::SessionInfo {
            session_id: "s".to_owned(),
            role,
            influx_token: None,
        });
        engine
    }

    fn detail_state() -> AppState {
        AppState {
            page: Page::NodeDetail,
            selected: Some(NodeKey::new(42, 4)),
            ..AppState::new()
        }
    }

    #[test]
    fn edit_opens_seeded_with_the_current_name() {
        let engine = engine(Some(UserRole::Admin));
        let mut state = detail_state();

        open_edit(&mut state, &engine, EditField::Name);

        assert_eq!(state.overlay, Some(Overlay::Edit(EditField::Name)));
        let edit = state.edit.expect("edit draft opened");
        assert_eq!(edit.input, "Line 3 sensor");
        assert_eq!(edit.original, "Line 3 sensor");
    }

    #[test]
    fn operators_cannot_open_the_editor() {
        let engine = engine(Some(UserRole::Operator));
        let mut state = detail_state();

        open_edit(&mut state, &engine, EditField::Name);

        assert_eq!(state.overlay, None);
        assert!(state.edit.is_none());
        assert_eq!(
            state.message.as_deref(),
            Some("editing requires an admin session")
        );
    }

    #[test]
    fn edit_without_a_session_is_refused() {
        let engine = engine(None);
        let mut state = detail_state();

        open_edit(&mut state, &engine, EditField::Name);

        assert_eq!(state.overlay, None);
        assert_eq!(
            state.message.as_deref(),
            Some("editing requires an admin session")
        );
    }

    #[test]
    fn edit_requires_a_selected_node() {
        let engine = engine(Some(UserRole::Admin));
        let mut state = AppState::new();

        open_edit(&mut state, &engine, EditField::Name);

        assert_eq!(state.overlay, None);
        assert_eq!(state.message.as_deref(), Some("no node selected"));
    }

    #[test]
    fn edit_input_and_backspace_modify_the_draft() {
        let engine = engine(Some(UserRole::Admin));
        let mut state = detail_state();

        open_edit(&mut state, &engine, EditField::Name);

        // clear the seeded name, then type a new one
        for _ in 0.."Line 3 sensor".len() {
            update(&mut state, Action::TextualBackspace, &engine);
        }
        for character in "Pump 3".chars() {
            update(&mut state, Action::TextualInput(character), &engine);
        }

        assert_eq!(state.edit.as_ref().expect("draft").input, "Pump 3");
    }

    #[test]
    fn tab_switches_field_and_reseeds_from_the_live_value() {
        let engine = engine(Some(UserRole::Admin));
        let mut state = detail_state();

        open_edit(&mut state, &engine, EditField::Name);
        update(&mut state, Action::CycleEditField, &engine);

        assert_eq!(state.overlay, Some(Overlay::Edit(EditField::Description)));
        let edit = state.edit.expect("draft");
        assert_eq!(edit.field, EditField::Description);
        assert_eq!(edit.input, "", "demo nodes have no description");
    }

    #[test]
    fn submit_emits_a_pending_edit_for_the_changed_field() {
        let engine = engine(Some(UserRole::Admin));
        let mut state = detail_state();

        open_edit(&mut state, &engine, EditField::Name);
        for _ in 0.."Line 3 sensor".len() {
            update(&mut state, Action::TextualBackspace, &engine);
        }
        for character in "Pump 3".chars() {
            update(&mut state, Action::TextualInput(character), &engine);
        }
        update(&mut state, Action::TextualSubmit, &engine);

        assert_eq!(state.overlay, None);
        assert!(state.edit.is_none());

        let pending = state.pending_edit.expect("a command was emitted");
        assert_eq!(pending.network_id, 42);
        assert_eq!(pending.address, 4);
        assert_eq!(
            pending.patches,
            vec![NodeMetadataPatch::Name("Pump 3".to_owned())]
        );
    }

    #[test]
    fn unchanged_submit_does_not_emit_a_command() {
        let engine = engine(Some(UserRole::Admin));
        let mut state = detail_state();

        open_edit(&mut state, &engine, EditField::Name);
        update(&mut state, Action::TextualSubmit, &engine);

        assert!(state.pending_edit.is_none());
        assert_eq!(state.message.as_deref(), Some("name unchanged"));
    }

    #[test]
    fn empty_name_submit_clears_the_field() {
        let engine = engine(Some(UserRole::Admin));
        let mut state = detail_state();

        open_edit(&mut state, &engine, EditField::Name);
        for _ in 0.."Line 3 sensor".len() {
            update(&mut state, Action::TextualBackspace, &engine);
        }
        update(&mut state, Action::TextualSubmit, &engine);

        let pending = state.pending_edit.expect("a command was emitted");
        assert_eq!(
            pending.patches,
            vec![NodeMetadataPatch::Name(String::new())]
        );
    }

    #[test]
    fn cancel_closes_the_editor_without_emitting() {
        let engine = engine(Some(UserRole::Admin));
        let mut state = detail_state();

        open_edit(&mut state, &engine, EditField::Name);
        update(&mut state, Action::TextualCancel, &engine);

        assert_eq!(state.overlay, None);
        assert!(state.edit.is_none());
        assert!(state.pending_edit.is_none());
        assert_eq!(state.page, Page::NodeDetail, "stays on detail");
    }

    #[test]
    fn palette_edit_opens_the_editor_for_the_selected_node() {
        let engine = engine(Some(UserRole::Admin));
        let mut state = detail_state();

        state.overlay = Some(Overlay::Palette);
        state.palette_input = "edit".to_owned();
        update(&mut state, Action::TextualSubmit, &engine);

        assert_eq!(state.overlay, Some(Overlay::Edit(EditField::Name)));
        assert_eq!(state.edit.expect("draft").input, "Line 3 sensor");
    }
}
