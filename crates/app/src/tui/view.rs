use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Row, Table},
};
use wntui_core::{
    engine::EngineState,
    model::{Node, Role},
};

use crate::tui::{
    app::{AppState, Page},
    detail::{detail_rows, network_label, status_word},
    listing::{NodeView, OVERSCAN, SortField, format_age, selection_index_in, window},
    theme::{self as color},
};

pub fn render(app: &AppState, engine: &EngineState, frame: &mut Frame) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    render_header(app, engine, frame, chunks[0]);
    render_filter_line(app, frame, chunks[1]);

    match app.page {
        Page::Nodes => render_nodes(app, engine, frame, chunks[2]),
        Page::Networks => render_networks(app, engine, frame, chunks[2]),
        Page::NodeDetail => render_node_detail(app, engine, frame, chunks[2]),
        Page::Graphs => super::graphs::render(app, engine, frame, chunks[2]),
    }

    match app.overlay {
        Some(super::app::Overlay::Help) => render_help(app, frame, area),
        Some(super::app::Overlay::Fields) => render_fields(frame, area),
        Some(super::app::Overlay::Palette) => render_palette(app, frame, area),
        Some(super::app::Overlay::Filter) => render_filter_editor(app, frame, area),
        Some(super::app::Overlay::Edit(field)) => render_edit_editor(app, field, frame, area),
        None => {}
    }

    render_status(app, engine, frame, chunks[3]);
}

fn render_header(app: &AppState, engine: &EngineState, frame: &mut Frame, area: Rect) {
    let title = Line::from(vec![
        Span::styled(
            " wntui",
            Style::default()
                .fg(color::PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            network_label(engine, app.selected_network),
            Style::default().fg(color::ACCENT),
        ),
        Span::raw("  "),
        Span::styled(
            connection_label(engine),
            Style::default().fg(connection_color(engine)),
        ),
    ]);
    frame.render_widget(title, area);
}

fn render_filter_line(app: &AppState, frame: &mut Frame, area: Rect) {
    if app.overlay != Some(super::app::Overlay::Filter) && app.filter_text.is_empty() {
        return;
    }

    let visible = app.projection.len();
    let total = app.projection.total();

    let mut spans = vec![
        Span::styled("/", Style::default().fg(color::PRIMARY)),
        Span::raw(if app.overlay == Some(super::app::Overlay::Filter) {
            app.filter_input.clone()
        } else {
            app.filter_text.clone()
        }),
        Span::styled(
            format!("  {visible} of {total}"),
            Style::default().fg(color::MUTED),
        ),
    ];

    if app.overlay == Some(super::app::Overlay::Filter) {
        spans.push(Span::styled(
            "  enter apply · esc cancel",
            Style::default().fg(color::DIM),
        ));
    }

    frame.render_widget(Line::from(spans), area);
}

// The filter editor: the expression line plus live validity feedback.
// Wireshark-style — [ok]/[err] and, on error, the message with a caret
// under the offending span. Tab suggestions show on their own line.
fn render_filter_editor(app: &AppState, frame: &mut Frame, area: Rect) {
    let popup = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: 1,
    };

    let empty = app.filter_input.trim().is_empty();
    let indicator = if empty {
        Span::styled("  ", Style::default())
    } else if app.filter_error.is_some() {
        Span::styled(" [err]", Style::default().fg(color::STATUS_OFFLINE))
    } else {
        Span::styled(" [ok]", Style::default().fg(color::STATUS_ONLINE))
    };

    let mut spans = vec![
        Span::styled("/", Style::default().fg(color::PRIMARY)),
        Span::raw(app.filter_input.clone()),
        Span::styled("█", Style::default().fg(color::ACCENT)),
        indicator,
    ];

    if let Some(suggestion) = &app.filter_suggestion {
        spans.push(Span::styled(
            format!("  {suggestion}"),
            Style::default().fg(color::MUTED),
        ));
    }

    frame.render_widget(ratatui::widgets::Clear, popup);
    frame.render_widget(Line::from(spans), popup);

    // The caret and message lines draw just below the expression; the page
    // behind them is still visible around the one-line popup.
    if let Some(error) = &app.filter_error {
        let caret_line = Rect {
            y: popup.y + 1,
            ..popup
        };
        // Byte offsets from the parser map 1:1 to columns for ASCII
        // field names and values; non-ASCII free text may drift by the
        // multi-byte width, which is acceptable for a caret hint.
        let leading = " ".repeat(error.position);
        let width = error.end.saturating_sub(error.position).max(1);
        let caret = format!("{leading}{} {}", "^".repeat(width), error);

        frame.render_widget(
            Line::from(Span::styled(
                caret,
                Style::default().fg(color::STATUS_OFFLINE),
            )),
            caret_line,
        );
    }
}

fn render_status(app: &AppState, engine: &EngineState, frame: &mut Frame, area: Rect) {
    let counts = status_counts(app);

    let mut spans = vec![
        Span::styled(
            format!(" {} ", app.mode_label()),
            Style::default().fg(color::PRIMARY),
        ),
        Span::styled("│", Style::default().fg(color::DIM)),
        Span::styled(
            format!(" {} ", connection_label(engine)),
            Style::default().fg(connection_color(engine)),
        ),
        Span::styled("│", Style::default().fg(color::DIM)),
        Span::styled(
            format!(" ●{} ", counts.online),
            Style::default().fg(color::STATUS_ONLINE),
        ),
        Span::styled(
            format!("◐{} ", counts.uncertain),
            Style::default().fg(color::STATUS_UNCERTAIN),
        ),
        Span::styled(
            format!("○{} ", counts.offline),
            Style::default().fg(color::STATUS_OFFLINE),
        ),
    ];

    let hint = app
        .message
        .clone()
        .unwrap_or_else(|| ":cmd  /filter  ?help  q quit".to_owned());
    spans.push(Span::styled(
        format!(
            "{hint:>width$}",
            width = area.width.saturating_sub(visible_width(&spans)) as usize
        ),
        Style::default().fg(color::MUTED),
    ));

    frame.render_widget(Line::from(spans), area);
}

fn visible_width(spans: &[Span<'_>]) -> u16 {
    spans.iter().map(|span| span.width() as u16).sum()
}

struct StatusCounts {
    online: usize,
    uncertain: usize,
    offline: usize,
}

fn status_counts(app: &AppState) -> StatusCounts {
    let (online, uncertain, offline) = app.projection.status_counts();
    StatusCounts {
        online,
        uncertain,
        offline,
    }
}

fn connection_label(engine: &EngineState) -> &'static str {
    match engine.connection() {
        wntui_core::engine::ConnectionState::Connecting => "connecting",
        wntui_core::engine::ConnectionState::Reconnecting => "reconnecting",
        wntui_core::engine::ConnectionState::Syncing => "syncing",
        wntui_core::engine::ConnectionState::Live => "live",
    }
}

fn connection_color(engine: &EngineState) -> ratatui::style::Color {
    match engine.connection() {
        wntui_core::engine::ConnectionState::Live => color::STATUS_ONLINE,
        wntui_core::engine::ConnectionState::Syncing => color::STATUS_UNCERTAIN,
        _ => color::STATUS_OFFLINE,
    }
}

// Renders only the visible window plus overscan (N2): rows outside the
// window are never built, so frame cost is bounded by the viewport, not
// the network size.
fn render_nodes(app: &AppState, engine: &EngineState, frame: &mut Frame, area: Rect) {
    if let Some(loading) = engine.store.loading() {
        let progress = if loading.expected_nodes > 0 {
            format!(
                "loading {}/{} nodes",
                loading.seen_nodes, loading.expected_nodes
            )
        } else {
            "waiting for node count".to_owned()
        };
        frame.render_widget(
            Line::from(Span::styled(
                format!(" {progress}"),
                Style::default().fg(color::STATUS_UNCERTAIN),
            )),
            area,
        );
        return;
    }

    let keys = app.projection.keys();

    if keys.is_empty() {
        let hint = if app.filter_text.is_empty() {
            " no nodes — press n to pick a network or :net to select one"
        } else {
            " no nodes match the filter — / to edit it"
        };
        frame.render_widget(
            Line::from(Span::styled(hint, Style::default().fg(color::MUTED))),
            area,
        );
        return;
    }

    // The table header consumes one row of the area.
    let body_height = area.height.saturating_sub(1) as usize;
    let selection = selection_index_in(keys, app.selected);
    let pane = window(
        keys.len(),
        selection,
        body_height,
        OVERSCAN,
        app.scroll_offset,
    );

    // Only rows from the top of the viewport down are built; ratatui paints
    // them starting at row 0. Rows above the offset are never rendered —
    // moving up bumps the reducer's offset instead.
    let view = NodeView::from_app(app);
    let visible = view.resolve(engine, &keys[pane.offset..pane.end]);
    let selected_row = selection.saturating_sub(pane.offset);

    // The active sort column carries a direction marker so the sort state
    // is readable from the header, not just the status toast.
    let marker = if app.sort_reverse { "▼" } else { "▲" };
    let header = Row::new([
        Cell::from(sort_header(
            "ST",
            marker,
            app.sort,
            app.sort_reverse,
            SortField::Status,
        )),
        Cell::from(sort_header(
            "ADDR",
            marker,
            app.sort,
            app.sort_reverse,
            SortField::Address,
        )),
        Cell::from(sort_header(
            "NAME",
            marker,
            app.sort,
            app.sort_reverse,
            SortField::Name,
        )),
        Cell::from(sort_header(
            "ROLE",
            marker,
            app.sort,
            app.sort_reverse,
            SortField::Role,
        )),
        Cell::from(sort_header(
            "VOLT",
            marker,
            app.sort,
            app.sort_reverse,
            SortField::Voltage,
        )),
        Cell::from(sort_header(
            "AGE",
            marker,
            app.sort,
            app.sort_reverse,
            SortField::Age,
        )),
    ])
    .style(Style::default().fg(color::DIM));

    let rows = visible.iter().map(|node| node_row(node, app.now_ms));

    let table = Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Length(7),
            Constraint::Min(10),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(5),
        ],
    )
    .header(header)
    .row_highlight_style(
        Style::default()
            .bg(color::SUBTLE_BACKGROUND)
            .fg(color::PRIMARY),
    )
    .highlight_symbol("❯ ");

    let mut table_state = ratatui::widgets::TableState::default().with_selected(Some(selected_row));
    frame.render_stateful_widget(table, area, &mut table_state);
}

// Column header with the sort marker when this column is the active sort.
fn sort_header(
    label: &str,
    marker: &str,
    sort: SortField,
    _reverse: bool,
    column: SortField,
) -> String {
    if sort == column {
        format!("{label}{marker}")
    } else {
        label.to_owned()
    }
}

fn node_row(node: &Node, now_ms: u64) -> Row<'static> {
    let status_style = Style::default().fg(color::status_color(node.online));

    Row::new(vec![
        Cell::from(Span::styled(
            format!(
                "{} {}",
                color::status_glyph(node.online),
                status_word(node.online)
            ),
            status_style,
        )),
        Cell::from(format!("0x{:04x}", node.key.address)),
        Cell::from(node.name.clone().unwrap_or_else(|| "-".to_owned())),
        Cell::from(role_label(node.role)),
        Cell::from(
            node.voltage
                .map(|voltage| format!("{voltage:.2}"))
                .unwrap_or_else(|| "-".to_owned()),
        ),
        Cell::from(format_age(now_ms, node.last_seen_ms)),
    ])
    .style(Style::default())
}

fn role_label(role: Role) -> &'static str {
    match role {
        Role::Sink => "sink",
        Role::Headnode => "head",
        Role::Subnode => "sub",
        Role::Unknown => "?",
    }
}

fn render_node_detail(app: &AppState, engine: &EngineState, frame: &mut Frame, area: Rect) {
    let Some(node) = app.selected.and_then(|key| engine.store.node(&key)) else {
        frame.render_widget(
            Line::from(Span::styled(
                " this node is no longer in the live state — press q to go back",
                Style::default().fg(color::MUTED),
            )),
            area,
        );
        return;
    };

    let rows = detail_rows(node, engine, app.now_ms);
    let cursor = app.detail_cursor.min(rows.len().saturating_sub(1));

    let lines = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut style = row.style();
            if index == cursor {
                style = style.add_modifier(Modifier::REVERSED);
            }
            row.text.clone().patch_style(style)
        })
        .collect::<Vec<_>>();

    // Keep the cursor on screen: scroll only as far as needed.
    let scroll = cursor
        .saturating_sub(area.height as usize / 2)
        .min(lines.len().saturating_sub(area.height as usize)) as u16;

    frame.render_widget(
        ratatui::widgets::Paragraph::new(lines).scroll((scroll, 0)),
        area,
    );
}

fn render_networks(app: &AppState, engine: &EngineState, frame: &mut Frame, area: Rect) {
    let entries = network_entries(engine, &app.net_query, app.selected_network, app.picker_row);

    // +1 skips the query line; the highlight follows picker_row.
    let list_row = (app.picker_row + 1).min(entries.len().saturating_sub(1));
    let mut list_state = ratatui::widgets::ListState::default().with_selected(Some(list_row));

    frame.render_stateful_widget(
        ratatui::widgets::List::new(entries).highlight_style(
            Style::default()
                .bg(color::SUBTLE_BACKGROUND)
                .fg(color::PRIMARY),
        ),
        area,
        &mut list_state,
    );
}

fn network_entries(
    engine: &EngineState,
    query: &str,
    selected: Option<u32>,
    picker_row: usize,
) -> Vec<Line<'static>> {
    let total = engine.store.node_count();
    let mut entries = vec![Line::from(vec![
        Span::styled(" filter: ", Style::default().fg(color::DIM)),
        Span::raw(query.to_owned()),
        Span::styled("█", Style::default().fg(color::ACCENT)),
        Span::styled(
            "  (type to search, enter selects)",
            Style::default().fg(color::MUTED),
        ),
    ])];

    super::update::network_matches(engine, query)
        .into_iter()
        .enumerate()
        .for_each(|(index, entry)| {
            let (id, name, in_network) = match entry {
                None => ("all".to_owned(), "all networks".to_owned(), total),
                Some(id) => {
                    let in_network = engine
                        .store
                        .nodes()
                        .filter(|node| node.key.network_id == id)
                        .count();
                    let network = engine.networks.iter().find(|network| network.id == id);
                    let name = match network {
                        Some(network) if !network.name.is_empty() => network.name.clone(),
                        _ => "(unnamed)".to_owned(),
                    };
                    (id.to_string(), name, in_network)
                }
            };

            let marker = if selected == entry {
                "  ✓"
            } else if index == picker_row {
                "  ❯"
            } else {
                ""
            };

            entries.push(Line::from(vec![
                Span::styled(format!("  {id:<6}"), Style::default().fg(color::ACCENT)),
                Span::raw(format!("{name}  ")),
                Span::styled(
                    format!("({in_network} nodes)"),
                    Style::default().fg(color::MUTED),
                ),
                Span::styled(marker.to_owned(), Style::default().fg(color::PRIMARY)),
            ]));
        });

    entries
}

fn help_lines(page: Page) -> Vec<Line<'static>> {
    let common = vec![
        Line::raw(" :             command palette"),
        Line::raw(" ?             help"),
    ];

    let mut lines = match page {
        Page::Nodes => vec![
            Line::raw(" j/k, ↓/↑     move"),
            Line::raw(" g / G         top / bottom"),
            Line::raw(" ctrl-d/u      half page down / up"),
            Line::raw(" ctrl-f/b      full page down / up"),
            Line::raw(" enter         open node detail"),
            Line::raw(" s / S         cycle sort / reverse"),
            Line::raw(" :sort f [asc|desc]  sort by field"),
            Line::raw(" /             filter nodes"),
            Line::raw(" tab           complete field or value"),
            Line::raw(" :fields       filter field reference"),
            Line::raw(" n             network picker"),
        ],
        Page::Networks => vec![
            Line::raw(" type          filter by name or id"),
            Line::raw(" j/k, ↓/↑     move"),
            Line::raw(" g / G         top / bottom"),
            Line::raw(" enter         select network"),
        ],
        Page::NodeDetail => vec![
            Line::raw(" j/k, ↓/↑     move the field cursor"),
            Line::raw(" g / G         top / bottom"),
            Line::raw(" ctrl-d/u      half page down / up"),
            Line::raw(" enter         edit field / open its graph"),
            Line::raw(" v             graphs for this node"),
        ],
        Page::Graphs => vec![
            Line::raw(" h/l, ←/→     pan the window (wheel too)"),
            Line::raw(" +/-           zoom in / out (ctrl+wheel too)"),
            Line::raw(" H / L         jump to window start / end"),
            Line::raw(" r             refetch the current window"),
            Line::raw(" t             cycle range 1h/6h/24h/7d"),
        ],
    };

    lines.extend(common);
    lines.push(Line::raw(""));
    lines.push(Line::raw(" q / Esc       back or quit"));
    lines
}

fn render_help(app: &AppState, frame: &mut Frame, area: Rect) {
    let lines = help_lines(app.page);
    let height = lines.len() as u16 + 2;
    let popup = centered(area, 50, height);
    let mut content = vec![Line::styled(
        format!(" wntui — {} keys", app.page),
        Style::default().fg(color::PRIMARY),
    )];
    content.extend(lines);

    clear_and_frame(frame, popup, " help ");
    frame.render_widget(
        ratatui::widgets::Paragraph::new(content),
        popup_inner(popup),
    );
}

// The field reference overlay: generated from the same table the parser
// uses, so it can't drift. One line per field: name, aliases, kind,
// comparators, example.
fn render_fields(frame: &mut Frame, area: Rect) {
    let mut content = vec![Line::from(vec![
        Span::styled(" field           ", Style::default().fg(color::PRIMARY)),
        Span::styled("comparators  ", Style::default().fg(color::PRIMARY)),
        Span::styled("example", Style::default().fg(color::PRIMARY)),
    ])];

    for entry in wntui_core::filter::Field::reference() {
        content.push(Line::from(vec![
            Span::styled(
                format!(" {:<15}", entry.name),
                Style::default().fg(color::ACCENT),
            ),
            Span::styled(
                format!("{:<12}", entry.comparators),
                Style::default().fg(color::MUTED),
            ),
            Span::raw(entry.example.to_owned()),
        ]));

        if !entry.enum_values.is_empty() {
            content.push(Line::from(Span::styled(
                format!("   values: {}", entry.enum_values.join(" | ")),
                Style::default().fg(color::DIM),
            )));
        }
    }

    content.push(Line::raw(""));
    content.push(Line::from(Span::styled(
        " tab completes fields and values · q closes",
        Style::default().fg(color::MUTED),
    )));

    let height = content.len() as u16 + 2;
    let popup = centered(area, 64, height);
    clear_and_frame(frame, popup, " filter fields ");
    frame.render_widget(
        ratatui::widgets::Paragraph::new(content),
        popup_inner(popup),
    );
}

fn render_palette(app: &AppState, frame: &mut Frame, area: Rect) {
    let popup = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(2),
        width: area.width,
        height: 1,
    };

    let line = Line::from(vec![
        Span::styled(":", Style::default().fg(color::PRIMARY)),
        Span::raw(app.palette_input.clone()),
        Span::styled("█", Style::default().fg(color::ACCENT)),
    ]);

    frame.render_widget(ratatui::widgets::Clear, popup);
    frame.render_widget(line, popup);
}

fn render_edit_editor(
    app: &AppState,
    field: crate::tui::app::EditField,
    frame: &mut Frame,
    area: Rect,
) {
    let Some(edit) = &app.edit else {
        return;
    };

    let popup = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(3),
        width: area.width,
        height: 1,
    };

    let line = Line::from(vec![
        Span::styled(
            format!(" edit {} (tab switch, esc cancel): ", field.label()),
            Style::default().fg(color::DIM),
        ),
        Span::raw(edit.input.clone()),
        Span::styled("█", Style::default().fg(color::ACCENT)),
    ]);

    frame.render_widget(ratatui::widgets::Clear, popup);
    frame.render_widget(line, popup);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

fn clear_and_frame(frame: &mut Frame, area: Rect, title: &str) {
    let block = ratatui::widgets::Block::bordered()
        .title(Span::styled(title, Style::default().fg(color::PRIMARY)))
        .border_style(Style::default().fg(color::DIM));
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(block, area);
}

fn popup_inner(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use wntui_core::{engine::EngineState, model::Network};

    const FIXED_NOW_MS: u64 = 1_700_000_005_000;

    fn demo_engine_state() -> EngineState {
        let mut engine = EngineState::default();
        engine.session = Some(wntui_core::session::SessionInfo {
            session_id: "demo".to_owned(),
            role: wntui_core::session::UserRole::Admin,
            influx_token: None,
        });
        engine.realtime_connected = true;
        engine.networks = vec![Network {
            id: 42,
            name: "Demo plant".to_owned(),
        }];
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));
        engine
    }

    fn app() -> AppState {
        AppState {
            now_ms: FIXED_NOW_MS,
            ..AppState::new()
        }
    }

    fn render_to_text(
        app: &AppState,
        engine: &EngineState,
    ) -> (String, Vec<ratatui::style::Color>) {
        // The render path is pure and reads the projection cache, so the
        // tests refresh a clone the same way the drive loop does.
        let mut app = app.clone();
        super::super::update::refresh_projection(&mut app, engine);

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(&app, engine, frame)).unwrap();

        let buffer = terminal.backend().buffer();
        let text = (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        let colors = buffer
            .content()
            .iter()
            .map(|cell| cell.fg)
            .collect::<Vec<_>>();

        (text, colors)
    }

    #[test]
    fn nodes_page_shows_demo_nodes_with_official_status_colors() {
        let engine = demo_engine_state();
        assert_eq!(
            engine.connection(),
            wntui_core::engine::ConnectionState::Live
        );

        let (text, colors) = render_to_text(&app(), &engine);

        assert!(text.contains("Sink A"));
        assert!(text.contains("Line 7 sensor"));
        assert!(text.contains("live"));
        assert!(text.contains("5s"));

        assert!(colors.contains(&color::STATUS_ONLINE));
        assert!(colors.contains(&color::STATUS_UNCERTAIN));
        assert!(colors.contains(&color::STATUS_OFFLINE));
    }

    #[test]
    fn filter_line_shows_the_match_count() {
        let engine = demo_engine_state();
        let mut state = app();
        state.filter_text = "status:offline".to_owned();
        state.filter = wntui_core::filter::parse(&state.filter_text).ok();

        let (text, _) = render_to_text(&state, &engine);
        assert!(text.contains("2 of 8"));
        assert!(text.contains("status:offline"));
    }

    #[test]
    fn filter_editor_shows_live_validity_and_caret() {
        let engine = demo_engine_state();

        let mut valid = app();
        valid.overlay = Some(super::super::app::Overlay::Filter);
        valid.filter_input = "status:online".to_owned();
        let (text, _) = render_to_text(&valid, &engine);
        assert!(text.contains("[ok]"));

        let mut invalid = app();
        invalid.overlay = Some(super::super::app::Overlay::Filter);
        invalid.filter_input = "role:banana".to_owned();
        invalid.filter_error = wntui_core::filter::parse_detailed(&invalid.filter_input).err();
        let (text, _) = render_to_text(&invalid, &engine);
        assert!(text.contains("[err]"));
        assert!(text.contains("^"));
        assert!(text.contains("bad value for role"));
    }

    #[test]
    fn fields_overlay_lists_the_reference_table() {
        let mut state = app();
        state.overlay = Some(super::super::app::Overlay::Fields);

        let (text, _) = render_to_text(&state, &demo_engine_state());
        assert!(text.contains("filter fields"));
        assert!(text.contains("voltage"));
        assert!(text.contains("online | offline | uncertain"));
        assert!(text.contains("addr"));
    }

    #[test]
    fn table_header_marks_the_active_sort_column() {
        let engine = demo_engine_state();

        let mut state = app();
        let (text, _) = render_to_text(&state, &engine);
        assert!(text.contains("ADDR▲"), "default sort is address ascending");

        state.sort = crate::tui::listing::SortField::Status;
        state.sort_reverse = true;
        let (text, _) = render_to_text(&state, &engine);
        assert!(text.contains("ST▼"));
    }

    #[test]
    fn status_counts_reflect_the_selected_network_only() {
        let mut other_network = demo_engine_state();
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|mut message| {
                message.network_id = Some(7);
                other_network.store.apply(&message);
            });

        let mut state = app();
        state.selected_network = Some(7);

        let (text, _) = render_to_text(&state, &other_network);
        assert!(text.contains("network 7"));
    }

    #[test]
    fn network_picker_lists_discovered_networks() {
        let mut state = app();
        state.page = Page::Networks;

        let (text, _) = render_to_text(&state, &demo_engine_state());
        assert!(text.contains("all networks"));
        assert!(text.contains("42"));
        assert!(text.contains("Demo plant"));
    }

    #[test]
    fn help_overlay_renders_keymap() {
        let mut state = app();
        state.overlay = Some(super::super::app::Overlay::Help);

        let (text, _) = render_to_text(&state, &demo_engine_state());
        assert!(text.contains("wntui — NODES keys"));
        assert!(text.contains("filter nodes"));
        assert!(text.contains("open node detail"));
    }

    #[test]
    fn help_is_contextual_per_page() {
        let engine = demo_engine_state();

        let mut nodes_help = app();
        nodes_help.overlay = Some(super::super::app::Overlay::Help);
        let (nodes_text, _) = render_to_text(&nodes_help, &engine);
        assert!(nodes_text.contains("cycle sort"));

        let mut detail_help = app();
        detail_help.page = Page::NodeDetail;
        detail_help.overlay = Some(super::super::app::Overlay::Help);
        let (detail_text, _) = render_to_text(&detail_help, &engine);
        assert!(detail_text.contains("wntui — NODE keys"));
        assert!(detail_text.contains("edit field / open its graph"));

        let mut networks_help = app();
        networks_help.page = Page::Networks;
        networks_help.overlay = Some(super::super::app::Overlay::Help);
        let (networks_text, _) = render_to_text(&networks_help, &engine);
        assert!(networks_text.contains("wntui — NETWORKS keys"));
        assert!(networks_text.contains("filter by name or id"));
    }

    #[test]
    fn edit_overlay_renders_the_field_hint_and_input() {
        let engine = demo_engine_state();
        let mut state = app();
        state.page = Page::NodeDetail;
        state.selected = Some(wntui_core::model::NodeKey::new(42, 4));
        state.overlay = Some(super::super::app::Overlay::Edit(
            super::super::app::EditField::Name,
        ));
        state.edit = Some(crate::tui::app::EditDraft {
            key: wntui_core::model::NodeKey::new(42, 4),
            field: super::super::app::EditField::Name,
            original: "Line 3 sensor".to_owned(),
            input: "Pump 3".to_owned(),
        });

        let (text, _) = render_to_text(&state, &engine);
        assert!(text.contains("edit name (tab switch, esc cancel):"));
        assert!(text.contains("Pump 3"));
        assert!(text.contains("EDIT"), "mode label reflects the editor");
    }

    #[test]
    fn nodes_view_snapshot() {
        let (text, _) = render_to_text(&app(), &demo_engine_state());
        insta::assert_snapshot!(text);
    }

    // 100k nodes through the real render path: only the window slice is
    // resolved, so the frame cost is bounded by the viewport, not the
    // network size (N2 smoke test).
    #[test]
    fn huge_network_renders_only_the_visible_window() {
        let mut engine = wntui_core::engine::EngineState::default();
        engine.realtime_connected = true;
        for address in 1..=100_000u32 {
            engine.store.upsert(wntui_core::model::Node {
                key: wntui_core::model::NodeKey::new(42, address),
                name: Some(format!("node {address}")),
                online: wntui_core::model::OnlineStatus::Online,
                ..wntui_core::model::Node::default()
            });
        }
        engine.store.mark_snapshot_complete();

        let mut state = app();
        state.selected = Some(wntui_core::model::NodeKey::new(42, 1));

        let (text, _) = render_to_text(&state, &engine);

        // The window shows the top of the list...
        assert!(text.contains("0x0001"));
        assert!(text.contains("node 1"));
        // ...and nothing from the middle or the end.
        assert!(!text.contains("0x5000"));
        assert!(!text.contains("node 100000"));

        // Scrolling to the end shows the last rows, never the first.
        let mut at_end = app();
        at_end.selected = Some(wntui_core::model::NodeKey::new(42, 100_000));
        let (tail, _) = render_to_text(&at_end, &engine);
        assert!(tail.contains("node 100000"));
        assert!(!tail.contains("node 1 "));
    }
}

#[cfg(test)]
mod detail_tests {
    use super::super::app::{AppState, Page};
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use wntui_core::model::NodeKey;

    fn detail_app(address: u32) -> AppState {
        AppState {
            page: Page::NodeDetail,
            now_ms: 1_700_000_005_000,
            selected: Some(NodeKey::new(42, address)),
            ..AppState::new()
        }
    }

    fn detail_text(app: &AppState, engine: &wntui_core::engine::EngineState) -> String {
        detail_text_with_height(app, engine, 60)
    }

    fn detail_text_with_height(
        app: &AppState,
        engine: &wntui_core::engine::EngineState,
        height: u16,
    ) -> String {
        let mut app = app.clone();
        super::super::update::refresh_projection(&mut app, engine);

        let mut terminal = Terminal::new(TestBackend::new(100, height)).unwrap();
        terminal.draw(|frame| render(&app, engine, frame)).unwrap();

        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn engine() -> wntui_core::engine::EngineState {
        let mut engine = wntui_core::engine::EngineState::default();
        engine.realtime_connected = true;
        engine.networks = vec![wntui_core::model::Network {
            id: 42,
            name: "Demo plant".to_owned(),
        }];
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));
        engine
    }

    #[test]
    fn detail_shows_identity_status_mesh_and_firmware() {
        let text = detail_text(&detail_app(2), &engine());

        assert!(text.contains("Conveyor head 1"));
        assert!(text.contains("identity"));
        assert!(text.contains("0x0002 (2)"));
        assert!(text.contains("status"));
        assert!(text.contains("mesh"));
        assert!(text.contains("0x0001 -45 dBm (next hop)"));
        assert!(text.contains("power & radio"));
        assert!(text.contains("2.4 GHz"));
        assert!(text.contains("firmware"));
        assert!(text.contains("5.2.1.0"));
        assert!(text.contains("travel time"));
        assert!(text.contains("min 48 · avg 120 · max 310 ms"));
    }

    #[test]
    fn sink_detail_includes_app_config() {
        let text = detail_text(&detail_app(1), &engine());

        assert!(text.contains("Sink A"));
        assert!(text.contains("app config (sink)"));
        assert!(text.contains("60 s"));
        assert!(text.contains("positioning"));
        assert!(text.contains("anchor"));
    }

    #[test]
    fn detail_cursor_scrolls_to_keep_the_row_visible() {
        let engine = engine();
        let mut app = detail_app(1);
        let top = detail_text_with_height(&app, &engine, 12);

        app.detail_cursor = 25;
        let deep = detail_text_with_height(&app, &engine, 12);

        assert_ne!(top, deep);
        assert!(top.contains("identity"));
        assert!(
            !deep.contains("identity"),
            "the view scrolled past the top section"
        );
    }

    #[test]
    fn missing_node_renders_a_fallback() {
        let mut app = detail_app(99);
        app.selected = Some(NodeKey::new(42, 99));

        let text = detail_text(&app, &engine());
        assert!(text.contains("no longer in the live state"));
    }

    #[test]
    fn detail_view_snapshot() {
        insta::assert_snapshot!(detail_text(&detail_app(2), &engine()));
    }
}

#[cfg(test)]
mod graph_view_tests {
    use super::super::app::{AppState, GraphRequest, Page};
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use wntui_core::model::NodeKey;
    use wntui_core::wire::influx::{GraphMetric, GraphRange, Series, SeriesPoint};

    fn demo_engine() -> wntui_core::engine::EngineState {
        let mut engine = wntui_core::engine::EngineState::default();
        engine.realtime_connected = true;
        engine.networks = vec![wntui_core::model::Network {
            id: 42,
            name: "Demo plant".to_owned(),
        }];
        wntui_core::engine::demo_network()
            .into_iter()
            .for_each(|message| engine.store.apply(&message));
        engine
    }

    fn graph_app(metric: GraphMetric, series: Vec<Series>) -> AppState {
        AppState {
            page: Page::Graphs,
            now_ms: 1_700_000_005_000,
            selected: Some(NodeKey::new(42, 4)),
            graph: Some(GraphRequest {
                key: NodeKey::new(42, 4),
                metric,
                range: GraphRange::Hour24,
                requested: None,
                fetched: (1_700_000_000.0, 1_700_000_005.0),
                window: None,
                series,
                loading: false,
                error: None,
            }),
            ..AppState::new()
        }
    }

    fn graph_text(app: &AppState, engine: &wntui_core::engine::EngineState) -> String {
        let mut app = app.clone();
        super::super::update::refresh_projection(&mut app, engine);

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(&app, engine, frame)).unwrap();

        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn series_of(values: &[f64]) -> Vec<Series> {
        vec![Series {
            name: "analytics_boot".to_owned(),
            columns: vec!["time".to_owned(), "mean".to_owned()],
            points: values
                .iter()
                .enumerate()
                .map(|(i, value)| SeriesPoint {
                    timestamp_ns: 1_700_000_000_000_000_000 + i as i64 * 1_800_000_000_000,
                    values: vec![serde_json::json!(value)],
                })
                .collect(),
        }]
    }

    #[test]
    fn loading_and_error_states_render_hints() {
        let engine = demo_engine();

        let mut loading = graph_app(GraphMetric::Voltage, Vec::new());
        loading.graph.as_mut().unwrap().loading = true;
        let text = graph_text(&loading, &engine);
        assert!(text.contains("loading…"));

        let mut failed = graph_app(GraphMetric::Voltage, Vec::new());
        failed.graph.as_mut().unwrap().loading = false;
        failed.graph.as_mut().unwrap().error = Some("connection refused".to_owned());
        let text = graph_text(&failed, &engine);
        assert!(text.contains("fetch failed: connection refused"));
    }

    #[test]
    fn empty_series_renders_the_no_data_hint() {
        let engine = demo_engine();
        let app = graph_app(GraphMetric::Voltage, Vec::new());
        let text = graph_text(&app, &engine);
        assert!(text.contains("no data in this range"));
    }

    #[test]
    fn no_graph_requested_renders_the_hint() {
        let engine = demo_engine();
        let mut app = AppState::new();
        app.page = Page::Graphs;
        let text = graph_text(&app, &engine);
        assert!(text.contains("no graph requested"));
    }

    #[test]
    fn chart_draws_the_voltage_series_with_the_header() {
        let engine = demo_engine();
        let values: Vec<f64> = (0..40).map(|i| 3.3 - i as f64 * 0.004).collect();
        let app = graph_app(GraphMetric::Voltage, series_of(&values));

        let text = graph_text(&app, &engine);
        assert!(text.contains("voltage"));
        assert!(text.contains("Line 3 sensor"));
        assert!(text.contains("0x0004"));
        assert!(text.contains("24h"));
        // y axis bounds are labelled
        assert!(text.contains("3.1"));
    }

    #[test]
    fn graphs_view_snapshot() {
        let engine = demo_engine();
        let values: Vec<f64> = (0..40).map(|i| 3.3 - i as f64 * 0.004).collect();
        let app = graph_app(GraphMetric::Voltage, series_of(&values));
        insta::assert_snapshot!(graph_text(&app, &engine));
    }

    // The WNT-style online timeline: a stepped band across the three
    // lanes, grid at nice boundaries, wall-clock labels under them (N3).
    #[test]
    fn online_timeline_snapshot() {
        let engine = demo_engine();
        // 24h at 30-minute samples: online for 8h, offline for 8h,
        // uncertain for the last 8h.
        let values: Vec<f64> = (0..48)
            .map(|i| match i / 16 {
                0 => 2.0,
                1 => 0.0,
                _ => 1.0,
            })
            .collect();
        let app = graph_app(GraphMetric::Online, series_of(&values));
        insta::assert_snapshot!(graph_text(&app, &engine));
    }

    // A pan/zoom window (N4) narrows the view; the header shows the
    // window span and the grid follows the window, not the fetched range.
    #[test]
    fn windowed_graph_shows_the_span_hint_and_narrows_the_axis() {
        let engine = demo_engine();
        let values: Vec<f64> = (0..48).map(|_| 2.0).collect();
        let mut app = graph_app(GraphMetric::Online, series_of(&values));
        app.graph.as_mut().unwrap().window = Some((1_700_000_000.0, 1_700_000_600.0));

        let text = graph_text(&app, &engine);

        assert!(
            text.contains("[h/l pan +- zoom] 10m"),
            "the header carries the window span: {text}"
        );
        // A 10-minute window labels minute boundaries, not hours.
        assert!(text.contains("22:1"));
    }
}
