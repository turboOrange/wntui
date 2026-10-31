use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType},
};
use wntui_core::{
    engine::EngineState,
    model::OnlineStatus,
    wire::influx::{GraphMetric, Series},
};

use crate::tui::{
    app::{AppState, GraphRequest},
    theme::{self as color},
};

pub fn render(app: &AppState, engine: &EngineState, frame: &mut Frame, area: Rect) {
    let Some(graph) = app.graph.as_ref() else {
        frame.render_widget(
            Line::from(Span::styled(
                " no graph requested — press v on a node detail or :graph <metric>",
                Style::default().fg(color::MUTED),
            )),
            area,
        );
        return;
    };

    let node = engine
        .store
        .node(&graph.key)
        .map(|node| node.name.clone().unwrap_or_default())
        .unwrap_or_default();

    let mut header_spans = vec![
        Span::styled(
            format!(" {} ", graph.metric.label()),
            Style::default()
                .fg(color::PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("{node} ")),
        Span::styled(
            format!("0x{:04x} ", graph.key.address),
            Style::default().fg(color::ACCENT),
        ),
        Span::styled(
            format!("[t] range {} ", graph.range.label()),
            Style::default().fg(color::MUTED),
        ),
    ];

    // A pan/zoom window narrower than the fetched range shows its span
    // (N4), so the operator sees what the axes cover.
    if let Some((start, end)) = graph.window {
        header_spans.push(Span::styled(
            format!(
                "[h/l pan +- zoom] {} ",
                window_span_label(start as i64, end as i64)
            ),
            Style::default().fg(color::MUTED),
        ));
    }
    let header = Line::from(header_spans);

    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    frame.render_widget(header, chunks[0]);

    let body = chunks[1];
    if let Some(error) = &graph.error {
        frame.render_widget(
            Line::from(Span::styled(
                format!(" fetch failed: {error}"),
                Style::default().fg(color::STATUS_OFFLINE),
            )),
            body,
        );
        return;
    }

    if graph.loading {
        frame.render_widget(
            Line::from(Span::styled(
                " loading…",
                Style::default().fg(color::STATUS_UNCERTAIN),
            )),
            body,
        );
        return;
    }

    let points = series_points(graph);
    if points.is_empty() {
        frame.render_widget(
            Line::from(Span::styled(
                " no data in this range",
                Style::default().fg(color::MUTED),
            )),
            body,
        );
        return;
    }

    // The online timeline is a stepped band (custom widget), every other
    // metric is a Chart line (N3). Both honor the display window (N4).
    if graph.metric == GraphMetric::Online {
        render_timeline(&points, graph.window, frame, body);
    } else {
        render_chart(graph, &points, frame, body);
    }
}

fn render_chart(graph: &GraphRequest, all_points: &[(f64, f64)], frame: &mut Frame, area: Rect) {
    // A display window (N4) overrides the x bounds and narrows the
    // plotted points — the y axis follows the visible data.
    let (x_bounds, points) = match graph.window {
        Some((start, end)) => ([start, end], windowed_points(all_points, (start, end))),
        None => {
            let (x_bounds, _) = chart_bounds(graph.metric, all_points);
            (x_bounds, all_points.to_vec())
        }
    };
    let (_, y_bounds) = chart_bounds(graph.metric, &points);
    let x_labels = x_axis_labels(x_bounds);
    let y_labels = y_axis_labels(graph.metric, y_bounds);

    // Grid lines at nice time boundaries go under the chart: the canvas
    // only paints the cells its data touches, so the grid survives between
    // points (N3).
    let graph_area = chart_graph_area(
        area,
        &y_labels,
        x_labels.first().map(Span::width).unwrap_or(0),
    );
    let axis = TimeAxis::new(x_bounds[0], x_bounds[1], graph_area.width);
    axis.paint_grid(frame, graph_area);

    let dataset = Dataset::default()
        .marker(ratatui::symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color::PRIMARY))
        .data(&points);

    let chart = Chart::new(vec![dataset])
        .x_axis(
            Axis::default()
                .style(Style::default().fg(color::DIM))
                .bounds(x_bounds)
                .labels(x_labels),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(color::DIM))
                .bounds(y_bounds)
                .title(Span::styled(
                    y_axis_label(graph.metric),
                    Style::default().fg(color::MUTED),
                ))
                .labels(y_labels),
        )
        .block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(color::DIM)),
        );

    frame.render_widget(chart, area);

    // The chart spaces its own x labels evenly; replace them with labels
    // aligned under the grid boundaries so position and time agree.
    let label_row = area.bottom().saturating_sub(1);
    let buf = frame.buffer_mut();
    for col in area.left()..area.right() {
        buf[(col, label_row)].set_symbol(" ");
    }
    axis.paint_labels(buf, label_row, graph_area.left(), area.right());
}

// Word labels for the three timeline lanes; "uncertain" is the widest.
const WORD_COL: u16 = 9;

// The online/offline timeline, WNT-style: a stepped band that moves
// between three lanes (offline bottom, uncertain middle, online top) as
// the node's state changes, colored with the status palette. Vertical
// grid lines sit at nice time boundaries with wall-clock labels under
// them, so "when was this node online" reads directly off the axis (N3).
fn render_timeline(
    points: &[(f64, f64)],
    window: Option<(f64, f64)>,
    frame: &mut Frame,
    area: Rect,
) {
    // The display window (N4) overrides the data extent; the band shows
    // empty columns where the window has no samples.
    let (x0, x1) = match window {
        Some(bounds) => bounds,
        None => (
            points.first().map(|p| p.0).unwrap_or(0.0),
            points.last().map(|p| p.0).unwrap_or(0.0),
        ),
    };
    let x1 = if x1 > x0 { x1 } else { x0 + 1.0 };

    if area.width < WORD_COL + 3 || area.height < 5 {
        frame.render_widget(
            Line::from(Span::styled(
                " timeline needs a larger window",
                Style::default().fg(color::MUTED),
            )),
            area,
        );
        return;
    }

    let axis_col = area.left() + WORD_COL;
    let left = axis_col + 1;
    let width = area.right() - left;
    let label_row = area.bottom() - 1;
    let axis_row = area.bottom() - 2;
    let top = area.top();
    let bottom = axis_row;
    let height = (bottom - top) as usize;

    let buf = frame.buffer_mut();

    // Axes: vertical word axis plus the horizontal time axis.
    for row in top..=axis_row {
        let cell = &mut buf[(axis_col, row)];
        cell.set_symbol(if row == axis_row { "┼" } else { "│" })
            .set_fg(color::DIM);
    }
    for col in left..area.right() {
        buf[(col, axis_row)].set_symbol("─").set_fg(color::DIM);
    }

    // Grid at nice time boundaries, drawn under the band.
    let axis = TimeAxis::new(x0, x1, width);
    axis.paint_grid_into(buf, Rect::new(left, top, width, bottom - top));

    // The band: one state per column, painted across its lane.
    let states = state_columns(points, x0, x1, width);
    let lanes = lanes(height);
    for (col, state) in states.iter().enumerate() {
        // None means no sample in the window yet: the column stays empty.
        let Some(value) = state else {
            continue;
        };
        let Some(status) = status_of(*value) else {
            continue;
        };
        let (start, end) = match status {
            OnlineStatus::Online => lanes.0,
            OnlineStatus::Uncertain => lanes.1,
            OnlineStatus::Offline => lanes.2,
        };
        for row in start..end {
            let cell = &mut buf[(left + col as u16, top + row as u16)];
            cell.set_symbol(" ").set_bg(color::status_color(status));
        }
    }

    // Lane words, right-aligned against the axis, colored to match.
    if height >= 3 {
        let words = [
            ("online", OnlineStatus::Online, lanes.0),
            ("uncertain", OnlineStatus::Uncertain, lanes.1),
            ("offline", OnlineStatus::Offline, lanes.2),
        ];
        for (word, status, (start, end)) in words {
            let row = top + ((start + end) / 2) as u16;
            let start_col = axis_col - word.len() as u16;
            for (i, ch) in word.chars().enumerate() {
                let cell = &mut buf[(start_col + i as u16, row)];
                cell.set_symbol(ch.to_string().as_str())
                    .set_fg(color::status_color(status));
            }
        }
    }

    axis.paint_labels(buf, label_row, left, area.right());
}

// (start, end) row ranges for the online/uncertain/offline lanes within a
// graph `height` rows tall, top to bottom. Windows under three rows can't
// show three lanes; all states share the full height there and color
// carries the distinction.
fn lanes(height: usize) -> ((usize, usize), (usize, usize), (usize, usize)) {
    if height < 3 {
        return ((0, height), (0, height), (0, height));
    }
    let third = height / 3;
    ((0, third), (third, 2 * third), (2 * third, height))
}

// The online series carries the proto enum: 0 offline, 1 uncertain,
// 2 online. Anything else is a data problem, not a fourth state.
fn status_of(value: f64) -> Option<OnlineStatus> {
    match value as i64 {
        0 => Some(OnlineStatus::Offline),
        1 => Some(OnlineStatus::Uncertain),
        2 => Some(OnlineStatus::Online),
        _ => None,
    }
}

// State per column with step semantics: each column carries the value of
// the last point at or before its center time, and None before the first
// point in the window (no data). One walk, ordered like the columns.
fn state_columns(points: &[(f64, f64)], x0: f64, x1: f64, width: u16) -> Vec<Option<f64>> {
    let mut states = vec![None; width as usize];
    if x1 <= x0 || width == 0 {
        return states;
    }
    let span = x1 - x0;

    let mut idx = 0;
    while idx < points.len() && points[idx].0 <= x0 {
        idx += 1;
    }
    let mut current = if idx > 0 {
        Some(points[idx - 1].1)
    } else {
        None
    };

    for (col, state) in states.iter_mut().enumerate() {
        let t = x0 + (col as f64 + 0.5) * span / f64::from(width);
        while idx < points.len() && points[idx].0 <= t {
            current = Some(points[idx].1);
            idx += 1;
        }
        *state = current;
    }
    states
}

// The boundary ladder for grid lines: minute, quarter hour, hour, six
// hours, day.
const TICK_STEPS: [i64; 5] = [60, 15 * 60, 3600, 6 * 3600, 86_400];

// The smallest step whose boundaries stay readable at the given width.
fn nice_step(span: i64, width: u16) -> i64 {
    const MIN_SPACING: i64 = 8;
    for &step in TICK_STEPS.iter() {
        let count = span / step + 1;
        if count * MIN_SPACING <= i64::from(width) {
            return step;
        }
    }
    TICK_STEPS[TICK_STEPS.len() - 1]
}

// Nice time boundaries within [x0, x1], aligned to the epoch (wall-clock
// :00/:15/:30/:45, hours, or midnight).
fn time_ticks(x0: i64, x1: i64, width: u16) -> Vec<i64> {
    if x1 <= x0 || width == 0 {
        return Vec::new();
    }
    let step = nice_step(x1 - x0, width);
    let mut first = x0.div_euclid(step) * step;
    if first < x0 {
        first += step;
    }

    let mut ticks = Vec::new();
    let mut t = first;
    while t <= x1 {
        ticks.push(t);
        t += step;
    }
    ticks
}

// Column of a boundary time under the inclusive x0 → 0, x1 → width-1
// mapping.
fn time_to_col(t: i64, x0: f64, x1: f64, width: u16) -> Option<u16> {
    if x1 <= x0 || width == 0 {
        return None;
    }
    let ratio = (t as f64 - x0) / (x1 - x0);
    if !(0.0..=1.0).contains(&ratio) {
        return None;
    }
    let col = (ratio * f64::from(width - 1)).round();
    Some(col as u16)
}

// Pan/zoom window math (N4). Pure functions over (window, fetched); the
// reducer decides policy. A move that stays inside the fetched bounds is
// answered with (window, false) — the zoom fast path — and one that
// leaves them with (window, true), meaning "fetch this window".

// Below one minute the grid ladder bottoms out; above the largest range
// (7d) there is nothing more to fetch.
pub const MIN_WINDOW_SPAN: f64 = 60.0;
pub const MAX_WINDOW_SPAN: f64 = 7.0 * 86_400.0;

// The fetched bounds are approximations of what the engine queried, so
// give the containment check a second of slack either side.
const CONTAINMENT_SLACK: f64 = 1.0;

pub fn window_in_fetched(window: (f64, f64), fetched: (f64, f64)) -> bool {
    window.0 >= fetched.0 - CONTAINMENT_SLACK && window.1 <= fetched.1 + CONTAINMENT_SLACK
}

// Shift the window by `direction` (−1 left, +1 right) times half its
// span. The shift is not clamped: moving outside the fetched bounds is
// the refetch signal.
pub fn pan_window(window: (f64, f64), _fetched: (f64, f64), direction: f64) -> ((f64, f64), bool) {
    let span = window.1 - window.0;
    let shift = span / 2.0 * direction;
    let moved = (window.0 + shift, window.1 + shift);
    (moved, !window_in_fetched(moved, _fetched))
}

// Zoom around the window's center: factor < 1 zooms in, > 1 zooms out.
// The span clamps to [MIN_WINDOW_SPAN, MAX_WINDOW_SPAN]; a clamped zoom
// that stays inside the fetched bounds never refetches.
pub fn zoom_window(window: (f64, f64), fetched: (f64, f64), factor: f64) -> ((f64, f64), bool) {
    let center = (window.0 + window.1) / 2.0;
    let span = ((window.1 - window.0) * factor).clamp(MIN_WINDOW_SPAN, MAX_WINDOW_SPAN);
    let zoomed = (center - span / 2.0, center + span / 2.0);
    (zoomed, !window_in_fetched(zoomed, fetched))
}

// Jump to the start or end of the fetched bounds, keeping the span.
pub fn jump_window(window: (f64, f64), fetched: (f64, f64), to_start: bool) -> (f64, f64) {
    let span = (window.1 - window.0).clamp(MIN_WINDOW_SPAN, MAX_WINDOW_SPAN);
    if to_start {
        (fetched.0, fetched.0 + span)
    } else {
        (fetched.1 - span, fetched.1)
    }
}

// "12m", "6h" — the span of a window, for the fetch toast and header.
pub fn window_span_label(start: i64, end: i64) -> String {
    let seconds = (end - start).max(0);
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

// The time axis of a graph: the data window plus the boundary ladder.
// Owns the tick math so grid lines and their labels always agree.
struct TimeAxis {
    x0: f64,
    x1: f64,
    width: u16,
    with_day: bool,
}

impl TimeAxis {
    fn new(x0: f64, x1: f64, width: u16) -> Self {
        Self {
            x0,
            x1,
            width,
            with_day: (x1 - x0) > 48.0 * 3600.0,
        }
    }

    fn ticks(&self) -> Vec<i64> {
        time_ticks(self.x0 as i64, self.x1 as i64, self.width)
    }

    fn col(&self, t: i64) -> Option<u16> {
        time_to_col(t, self.x0, self.x1, self.width)
    }

    // Vertical grid lines through the plot area at each boundary.
    fn paint_grid(&self, frame: &mut Frame, area: Rect) {
        self.paint_grid_into(frame.buffer_mut(), area);
    }

    fn paint_grid_into(&self, buf: &mut Buffer, area: Rect) {
        for tick in self.ticks() {
            let Some(col) = self.col(tick) else {
                continue;
            };
            for row in area.top()..area.bottom() {
                buf[(area.left() + col, row)]
                    .set_symbol("│")
                    .set_fg(color::DIM);
            }
        }
    }

    // Wall-clock labels centered under their boundary columns, dropped
    // when they would collide with the previous label or the edge.
    fn paint_labels(&self, buf: &mut Buffer, row: u16, left: u16, right: u16) {
        let mut next_free = left;
        for tick in self.ticks() {
            let Some(col) = self.col(tick) else {
                continue;
            };
            let label = format_unix_time(tick, self.with_day);
            let start = (left + col)
                .saturating_sub(label.len() as u16 / 2)
                .max(next_free);
            let end = start + label.len() as u16;
            if end > right {
                continue;
            }
            for (i, ch) in label.chars().enumerate() {
                buf[(start + i as u16, row)]
                    .set_symbol(ch.to_string().as_str())
                    .set_fg(color::MUTED);
            }
            next_free = end + 1;
        }
    }
}

// Replicates the Chart widget's internal layout for the fixed
// configuration used here (x and y labels present, default left
// alignment, a LEFT-bordered block) so the grid can be painted under the
// plot area. Ratatui's layout is private and starts from the block's
// inner area; a test pins this against the real widget.
fn chart_graph_area(
    chart_area: Rect,
    y_labels: &[Span<'static>],
    first_x_label_width: usize,
) -> Rect {
    // The block carries a LEFT border: the layout starts one column in.
    let inner_left = chart_area.left() + 1;
    let inner_width = chart_area.width.saturating_sub(1);

    let y_width = y_labels.iter().map(Span::width).max().unwrap_or(0) as u16;
    let label_col = y_width
        .max(first_x_label_width.saturating_sub(1) as u16)
        .min(inner_width / 3);
    let left = inner_left + label_col + 1;
    let bottom = chart_area.bottom().saturating_sub(2);
    Rect::new(
        left,
        chart_area.top(),
        chart_area.right().saturating_sub(left),
        bottom.saturating_sub(chart_area.top()),
    )
}

// Online status is the proto enum (0 = offline, 1 = uncertain, 2 = online),
// so its axis reads words instead of raw numbers; every other metric keeps
// its numeric labels.
fn y_axis_labels(metric: GraphMetric, bounds: [f64; 2]) -> Vec<Span<'static>> {
    if metric == GraphMetric::Online {
        return ["offline", "uncertain", "online"]
            .into_iter()
            .map(|word| Span::styled(word.to_owned(), Style::default().fg(color::MUTED)))
            .collect();
    }

    vec![
        Span::styled(
            format!("{:.1}", bounds[0]),
            Style::default().fg(color::MUTED),
        ),
        Span::styled(
            format!("{:.1}", bounds[1]),
            Style::default().fg(color::MUTED),
        ),
    ]
}

// Three labels across the fetched window: start, middle, end. Times are
// wall-clock (UTC) — x is absolute unix seconds, not "seconds from the
// first point", so the axis answers "when was this".
fn x_axis_labels(bounds: [f64; 2]) -> Vec<Span<'static>> {
    let span = bounds[1] - bounds[0];
    let with_day = span > 48.0 * 3600.0;

    [bounds[0], (bounds[0] + bounds[1]) / 2.0, bounds[1]]
        .iter()
        .map(|seconds| {
            Span::styled(
                format_unix_time(*seconds as i64, with_day),
                Style::default().fg(color::MUTED),
            )
        })
        .collect()
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn format_unix_time(unix_seconds: i64, with_day: bool) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let time_of_day = unix_seconds.rem_euclid(86_400);

    let (_, month, day) = civil_from_days(days);
    let (hour, minute) = (time_of_day / 3600, (time_of_day % 3600) / 60);

    if with_day {
        format!(
            "{} {:02} {:02}:{:02}",
            MONTHS[(month - 1) as usize],
            day,
            hour,
            minute
        )
    } else {
        format!("{hour:02}:{minute:02}")
    }
}

// Days since 1970-01-01 to (year, month, day) — Howard Hinnant's
// civil_from_days; UTC only, no local time.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 - doe / 36_524 + doe / 146_096) / 365;
    let year = yoe + era * 400;
    let day_of_year = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

// Points inside the window plus one neighbor on each side, so chart
// lines enter and leave the view instead of stopping at the edge.
fn windowed_points(points: &[(f64, f64)], window: (f64, f64)) -> Vec<(f64, f64)> {
    let first = points
        .partition_point(|(t, _)| *t < window.0)
        .saturating_sub(1);
    let last = (points.partition_point(|(t, _)| *t <= window.1) + 1).min(points.len());
    points[first..last].to_vec()
}

// The first value column of the first series, projected to chart points
// (x = seconds from the first timestamp, y = value).
fn series_points(graph: &GraphRequest) -> Vec<(f64, f64)> {
    graph
        .series
        .first()
        .map(series_points_of)
        .unwrap_or_default()
}

// The first value column of the first series, projected to chart points
// (x = absolute unix seconds, y = value). Absolute time keeps the x axis
// meaningful and lets range windows merge later.
fn series_points_of(series: &Series) -> Vec<(f64, f64)> {
    series
        .points
        .iter()
        .filter_map(|point| {
            let value = point.values.first()?.as_f64()?;
            let seconds = point.timestamp_ns as f64 / 1_000_000_000.0;
            Some((seconds, value))
        })
        .collect()
}

fn chart_bounds(metric: GraphMetric, points: &[(f64, f64)]) -> ([f64; 2], [f64; 2]) {
    let x_min = points.first().map(|p| p.0).unwrap_or(0.0);
    let x_max = points.last().map(|p| p.0).unwrap_or(0.0);
    let y_min = points.iter().fold(f64::INFINITY, |acc, p| acc.min(p.1));
    let y_max = points.iter().fold(f64::NEG_INFINITY, |acc, p| acc.max(p.1));

    // The online enum is ordinal (offline/uncertain/online); pin the axis to
    // the full enum range so a flat "always online" series doesn't zoom in
    // on a meaningless band.
    if metric == GraphMetric::Online {
        return ([x_min, x_max], [0.0, 2.0]);
    }

    // Pad y so flat series (boot counts) still draw.
    let (y_min, y_max) = if (y_max - y_min).abs() < f64::EPSILON {
        (y_min - 0.5, y_max + 0.5)
    } else {
        (y_min, y_max)
    };

    ([x_min, x_max], [y_min, y_max])
}

fn y_axis_label(metric: GraphMetric) -> &'static str {
    match metric {
        GraphMetric::Voltage => "V",
        GraphMetric::TraveltimeQos0 | GraphMetric::TraveltimeQos1 => "ms",
        GraphMetric::Online | GraphMetric::Boots | GraphMetric::Packets => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_time_formats_as_hours_and_minutes() {
        // 2023-11-14 22:13:20 UTC
        assert_eq!(format_unix_time(1_700_000_000, false), "22:13");
    }

    #[test]
    fn windows_longer_than_two_days_carry_the_date() {
        // 2023-11-14 22:13:20 UTC
        assert_eq!(format_unix_time(1_700_000_000, true), "Nov 14 22:13");
    }

    #[test]
    fn axis_labels_cover_start_middle_and_end() {
        let labels = x_axis_labels([1_700_000_000.0, 1_700_086_400.0]);
        let rendered: Vec<String> = labels.iter().map(|span| span.content.to_string()).collect();
        assert_eq!(rendered.len(), 3);
        assert!(rendered.iter().all(|label| label.contains(':')));
    }

    #[test]
    fn epoch_and_day_boundaries_format_cleanly() {
        assert_eq!(format_unix_time(0, false), "00:00");
        assert_eq!(format_unix_time(86_400 * 40 + 3600, true), "Feb 10 01:00");
    }

    #[test]
    fn nice_step_grows_with_the_span_and_the_width() {
        // (span, width, expected step)
        let cases: &[(i64, u16, i64)] = &[
            (3600, 85, 900),           // 1h: quarter-hour boundaries
            (6 * 3600, 85, 3600),      // 6h: hourly boundaries
            (24 * 3600, 85, 6 * 3600), // 24h: six-hour boundaries
            (7 * 86_400, 85, 86_400),  // 7d: daily boundaries
            (3600, 20, 3600),          // narrow window: hourly
            (60, 85, 60),              // tiny span: per-minute
        ];

        for &(span, width, expected) in cases {
            assert_eq!(
                nice_step(span, width),
                expected,
                "span={span} width={width}"
            );
        }
    }

    #[test]
    fn time_ticks_align_to_wall_clock_boundaries() {
        // 22:13:20 to 23:13:20 UTC with quarter-hour steps: the last
        // boundary inside the window is 23:00 (= start + 2800s).
        let start = 1_700_000_000;
        let ticks = time_ticks(start, start + 3600, 85);

        assert_eq!(ticks.first(), Some(&1_700_000_100)); // 22:15
        assert_eq!(ticks.last(), Some(&1_700_002_800)); // 23:00
        // Every tick sits on a quarter-hour boundary.
        assert!(ticks.iter().all(|t| t.rem_euclid(900) == 0));
    }

    #[test]
    fn time_to_col_maps_the_window_edges_exactly() {
        assert_eq!(time_to_col(100, 100.0, 200.0, 50), Some(0));
        assert_eq!(time_to_col(200, 100.0, 200.0, 50), Some(49));
        // 0.5 * 49 = 24.5 rounds half away from zero.
        assert_eq!(time_to_col(150, 100.0, 200.0, 50), Some(25));
        assert_eq!(time_to_col(99, 100.0, 200.0, 50), None);
        assert_eq!(time_to_col(201, 100.0, 200.0, 50), None);
    }

    #[test]
    fn state_columns_follow_step_semantics() {
        let points = vec![(10.0, 2.0), (30.0, 0.0), (60.0, 1.0)];

        // Window starts at the first point: the leading state applies.
        let states = state_columns(&points, 10.0, 70.0, 60);
        assert_eq!(states[0], Some(2.0)); // first column: online
        assert_eq!(states[19], Some(2.0)); // just before the offline point
        assert_eq!(states[20], Some(0.0)); // offline takes over
        assert_eq!(states[49], Some(0.0));
        assert_eq!(states[50], Some(1.0)); // uncertain to the end
        assert_eq!(states[59], Some(1.0));

        // Window starts before the first point: leading columns carry no
        // data and stay empty.
        let states = state_columns(&points, 0.0, 70.0, 70);
        assert_eq!(states[0], None);
        assert_eq!(states[9], None);
        assert_eq!(states[10], Some(2.0));
    }

    #[test]
    fn lanes_split_the_graph_into_three_bands() {
        let (online, uncertain, offline) = lanes(9);
        assert_eq!(online, (0, 3));
        assert_eq!(uncertain, (3, 6));
        assert_eq!(offline, (6, 9));

        // Remainder rows land in the offline band; tiny windows collapse
        // to a single full-height band.
        let (online, _, offline) = lanes(10);
        assert_eq!(online, (0, 3));
        assert_eq!(offline, (6, 10));
        let (online, _, _) = lanes(2);
        assert_eq!(online, (0, 2));
    }

    // Renders the online timeline into a real buffer and inspects it:
    // band colors by lane, grid lines, word labels, boundary labels.
    #[test]
    fn timeline_renders_the_stepped_band_with_grid_and_labels() {
        use ratatui::{Terminal, backend::TestBackend};

        // Two hours of states: online for the first 40 minutes, offline
        // for the next 40, uncertain for the last 40.
        let base = 1_700_000_000.0; // 22:13:20
        let points = vec![
            (base, 2.0),
            (base + 2400.0, 0.0),
            (base + 4800.0, 1.0),
            (base + 7200.0, 1.0),
        ];

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| {
                render_timeline(&points, None, frame, Rect::new(0, 0, 100, 30));
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let text: String = (0..buffer.area.height)
            .flat_map(|row| (0..buffer.area.width).map(move |col| (col, row)))
            .map(|(col, row)| buffer[(col, row)].symbol().to_owned())
            .collect();

        // The three lane words are present and colored.
        assert!(text.contains("online"));
        assert!(text.contains("uncertain"));
        assert!(text.contains("offline"));

        // Grid lines at quarter-hour boundaries run through the plot.
        assert!(text.matches('│').count() > 3);

        // Wall-clock labels under the boundaries.
        assert!(text.contains("22:15"));
        assert!(text.contains("23:00"));

        // The band: the online segment paints STATUS_ONLINE backgrounds in
        // the top lane, the offline segment STATUS_OFFLINE in the bottom
        // lane, and uncertain STATUS_UNCERTAIN in between.
        let bgs: std::collections::HashSet<ratatui::style::Color> =
            buffer.content().iter().map(|cell| cell.bg).collect();
        assert!(bgs.contains(&color::STATUS_ONLINE));
        assert!(bgs.contains(&color::STATUS_UNCERTAIN));
        assert!(bgs.contains(&color::STATUS_OFFLINE));

        // The step is visible: a column in the online segment has a green
        // top lane and no red cells; a column in the offline segment has a
        // red bottom lane and no green cells.
        let graph_left = WORD_COL + 1;
        let top_lane_row = 1; // lanes(24ish) → online band at the top
        let bottom_lane_row = 22;
        let online_col = graph_left + 5; // within the first 40 minutes
        let offline_col = graph_left + 45; // within the second 40 minutes
        assert_eq!(buffer[(online_col, top_lane_row)].bg, color::STATUS_ONLINE);
        assert_eq!(
            buffer[(offline_col, bottom_lane_row)].bg,
            color::STATUS_OFFLINE
        );
    }

    // Pins chart_graph_area against the real Chart widget: the axes the
    // chart draws must sit exactly where the replication says.
    #[test]
    fn chart_graph_area_matches_the_real_chart_layout() {
        use ratatui::{
            Terminal,
            backend::TestBackend,
            widgets::{Block, Borders},
        };

        let area = Rect::new(0, 0, 100, 26);
        let x_bounds = [1_700_000_000.0, 1_700_086_400.0];
        let y_bounds = [0.0, 5.0];
        let x_labels = x_axis_labels(x_bounds);
        let y_labels = vec![
            Span::styled("0.0".to_owned(), Style::default()),
            Span::styled("5.0".to_owned(), Style::default()),
        ];

        let expected = chart_graph_area(
            area,
            &y_labels,
            x_labels.first().map(Span::width).unwrap_or(0),
        );

        let data = vec![(x_bounds[0], 0.0), (x_bounds[1], 5.0)];

        let dataset = Dataset::default()
            .marker(ratatui::symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(color::PRIMARY))
            .data(&data);

        let chart = Chart::new(vec![dataset])
            .x_axis(
                Axis::default()
                    .style(Style::default().fg(color::DIM))
                    .bounds(x_bounds)
                    .labels(x_labels),
            )
            .y_axis(
                Axis::default()
                    .style(Style::default().fg(color::DIM))
                    .bounds(y_bounds)
                    .labels(y_labels),
            )
            .block(
                Block::default()
                    .borders(Borders::LEFT)
                    .border_style(Style::default().fg(color::DIM)),
            );

        let mut terminal = Terminal::new(TestBackend::new(100, 26)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(chart, area))
            .unwrap();

        // Find the axis corner `└`: its column is the y axis, its row the
        // x axis. The graph area starts right of the axis and ends above it.
        let buffer = terminal.backend().buffer();
        let mut corner = None;
        for row in 0..buffer.area.height {
            for col in 0..buffer.area.width {
                if buffer[(col, row)].symbol() == "└" {
                    corner = Some((col, row));
                }
            }
        }

        let (axis_col, axis_row) = corner.expect("the chart drew its axis corner");
        assert_eq!(expected.left(), axis_col + 1);
        assert_eq!(expected.bottom(), axis_row);
    }

    // The grid is painted under the chart; the chart's braille canvas only
    // touches its data cells, so grid lines survive between points.
    #[test]
    fn grid_lines_survive_under_the_chart() {
        use ratatui::{Terminal, backend::TestBackend};

        let area = Rect::new(0, 0, 100, 26);
        let x_bounds = [1_700_000_000.0, 1_700_086_400.0];

        let mut terminal = Terminal::new(TestBackend::new(100, 26)).unwrap();
        terminal
            .draw(|frame| {
                let y_labels = y_axis_labels(GraphMetric::Voltage, [0.0, 5.0]);
                let graph_area = chart_graph_area(
                    area,
                    &y_labels,
                    format_unix_time(x_bounds[0] as i64, false).len(),
                );
                TimeAxis::new(x_bounds[0], x_bounds[1], graph_area.width)
                    .paint_grid(frame, graph_area);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let grid: Vec<(u16, u16)> = (0..buffer.area.height)
            .flat_map(|row| (0..buffer.area.width).map(move |col| (col, row)))
            .filter(|&(col, row)| buffer[(col, row)].symbol() == "│")
            .collect();
        assert!(grid.len() > 10, "grid lines painted through the plot");
    }

    #[test]
    fn pan_keeps_the_window_inside_fetched_until_it_leaves() {
        let fetched = (100.0, 500.0);

        // Pan inside: just moves, no refetch (the fast path).
        let (window, leaves) = pan_window((100.0, 300.0), fetched, 1.0);
        assert_eq!(window, (200.0, 400.0));
        assert!(!leaves);

        let (window, leaves) = pan_window((300.0, 500.0), fetched, -1.0);
        assert_eq!(window, (200.0, 400.0));
        assert!(!leaves);

        // Pan past the edge: the window leaves and signals a refetch.
        let (window, leaves) = pan_window((300.0, 500.0), fetched, 1.0);
        assert_eq!(window, (400.0, 600.0));
        assert!(leaves);
    }

    #[test]
    fn zoom_in_never_refetches_and_zoom_out_eventually_does() {
        let fetched = (100.0, 500.0);

        // Zoom in: the window shrinks inside the fetched data.
        let (window, leaves) = zoom_window((100.0, 500.0), fetched, 0.5);
        assert_eq!(window, (200.0, 400.0));
        assert!(!leaves);

        // Zoom out past the fetched bounds: refetch the wider window.
        let (window, leaves) = zoom_window((200.0, 400.0), fetched, 2.0);
        assert_eq!(window, (100.0, 500.0));
        assert!(!leaves, "zooming back out onto the fetched span stays");

        let (window, leaves) = zoom_window((100.0, 500.0), fetched, 2.0);
        assert_eq!(window, (-100.0, 700.0));
        assert!(leaves);
    }

    #[test]
    fn zoom_respects_the_span_limits() {
        // Zooming in never goes below one minute of span.
        let tiny = (1_000.0, 1_000.0 + MIN_WINDOW_SPAN / 4.0);
        let (window, _) = zoom_window(tiny, (0.0, 10_000.0), 0.5);
        assert!((window.1 - window.0 - MIN_WINDOW_SPAN).abs() < 1e-6);

        // Zooming out never exceeds the largest range, 7d.
        let wide = (0.0, MAX_WINDOW_SPAN);
        let (window, _) = zoom_window(wide, (0.0, MAX_WINDOW_SPAN * 4.0), 2.0);
        assert!((window.1 - window.0 - MAX_WINDOW_SPAN).abs() < 1e-6);
    }

    #[test]
    fn jumps_pin_the_window_to_the_fetched_edges() {
        let fetched = (100.0, 500.0);

        let window = jump_window((400.0, 500.0), fetched, true);
        assert_eq!(window, (100.0, 200.0));

        let window = jump_window((100.0, 200.0), fetched, false);
        assert_eq!(window, (400.0, 500.0));
    }

    #[test]
    fn span_labels_read_compactly() {
        assert_eq!(window_span_label(0, 45), "45s");
        assert_eq!(window_span_label(0, 720), "12m");
        assert_eq!(window_span_label(0, 7 * 3600), "7h");
        assert_eq!(window_span_label(0, 3 * 86_400), "3d");
    }

    #[test]
    fn windowed_points_keep_one_neighbor_beyond_each_edge() {
        let points: Vec<(f64, f64)> = (0..=10).map(|t| (t as f64 * 10.0, 1.0)).collect();

        // Window (25, 65): points 30..60 are inside; 20 and 70 ride along
        // so the chart line enters and leaves the view.
        let visible = windowed_points(&points, (25.0, 65.0));
        let times: Vec<f64> = visible.iter().map(|p| p.0).collect();
        assert_eq!(times, vec![20.0, 30.0, 40.0, 50.0, 60.0, 70.0]);

        // A window between two points still shows the crossing line.
        let visible = windowed_points(&points, (43.0, 47.0));
        assert_eq!(visible.len(), 2);

        // The full window returns everything.
        let visible = windowed_points(&points, (-10.0, 1000.0));
        assert_eq!(visible.len(), points.len());
    }
}
