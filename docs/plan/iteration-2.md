# Iteration 2 — plan

The three big items from the first live review of iteration 1, plus the
groundwork they need. Everything else from that review went into the
iteration-1 M10 fixpack (`iteration-1.md`).

## Status: N2 done (N3–N4 next)

N2 landed (2026-10-26): the node list is virtualized. A versioned
projection cache (`NodeStore.version` bumps on every mutation;
`Projection` in `tui/listing.rs` recomputes only when the store version,
filter text, sort, network, or — for `age:` filters only — the wall-clock
second bucket moved) feeds a windowed renderer that builds only the
visible rows plus a 10-row overscan below the viewport. The scroll offset
lives in `AppState` and is synced by `refresh_projection` in the reducer,
so scroll-under-the-cursor is sticky and render stays pure. The filter
line's "N of M" total and the status-bar counts come from the same cache
walk — no per-frame store scan. Verified by a 100k-node TestBackend render
test asserting only the window slice reaches the screen (top-of-list and
end-of-list cases), plus table-driven window-math and cache-invalidation
tests.

Extras that landed between iterations (user requests, not N-milestones):

- `wntui auth login|logout|status` — OS-keyring credential storage
  (zbus Secret Service, pure Rust), verified against the backend before
  storing; launch resolution keyring → `WNT_PASSWORD` → prompt.
  Closes the "plaintext in a file" concern from iteration 1's config.
- Online status is a graphable detail field (purple row, enter opens
  the `analytics_nodestate` timeline; word-labeled y axis pinned to
  0–2). The stepped WNT-style band rendering remains N3.
- Sort fields gained status and role; `:sort <field> [asc|desc]` palette
  command pre-lands the palette half of N1's sort row.
- Filter comparisons on `addr`/`net` (`addr:>0x2 and addr:<0x9`).

N1 landed (2026-10-26): located parse errors (`parse_detailed` carries
byte spans), `[ok]`/`[err]` with caret line in the filter overlay,
Tab completion for fields/operators/enum values (`tui/complete.rs`),
`:fields` reference overlay generated from the parser's field table
(`Field::reference()`), `▲`/`▼` sort markers on the table header, and
named filter profiles (`:filter-save`/`:filter-load`/`:filters`,
persisted via `Config::save()`). Known trade-off: `filter-save`
rewrites the config without preserving comments; caret columns assume
ASCII input.

Per-milestone audit (grep-verified, not assumed):

| Item | Verified present | Verified missing |
|---|---|---|
| N1 filters | `:sort` palette cmd; grammar core; located errors + caret rendering; `[ok]`/`[err]`; `:fields` overlay; Tab completion; sort direction in the header; named filter profiles | — |
| N2 virtualized list | projection cache keyed on store version + filter text + sort + network (+ second bucket for `age:`); windowed render (`render_nodes` builds only `[offset, end)`); 10-row overscan; `NodeStore::mark_snapshot_complete` for bulk fixtures; 100k TestBackend render test; `AppState.scroll_offset` synced by the reducer | per-frame store walks (gone from filter line, status counts, and table rows) |
| N3 graphs/timeline | HH:MM axis labels (F5); online metric + word-labeled pinned y axis | stepped band widget (online renders as a Chart line); nice-boundary grid lines |
| N4 pan/zoom | stale-reply guard by (key, metric, range) | pan/zoom keys (none bound on graphs page); window in `FetchGraph` (key/metric/range only, no start/end); zoom fast path |

## Goal

wntui stays fast and legible on a production network with 100k+ nodes,
graphs become explorable (pan, zoom, real time axes, WNT-style online
timeline), and the filter system feels like Wireshark: a display-filter
grammar the operator can read, edit, and get help on — not a fuzzy text
box.

## In scope (iteration 2)

1. Interactive graphs: pan and zoom, real time axis, WNT-style
   online/offline timeline as a first-class metric.
2. Virtualized node list: render only the visible window plus a small
   overscan; filter/sort projection cached, not recomputed per frame.
3. Wireshark-feel filtering and sorting: field reference, expression
   errors surfaced inline, sort by clicking column headers (or
   equivalents), autocomplete for fields and values.

## Out of scope (still)

- Floor plans / map.
- OTAP, downlink, remote API — unchanged from iteration 1.
- 3rd-party charting / tui-wrapper crates; ratatui `Chart` and custom
  widgets remain the rendering layer.

## 1. Interactive graphs

### Metrics

Match the WNT official client's node history view:

| Metric | Notes |
|---|---|
| Online timeline | stepped band (online/uncertain/offline over time), WNT-style; states colored with the existing status palette |
| Voltage | line |
| Travel time (QoS0/QoS1) | line |
| Boot events | markers on the time axis |
| Packet rate | line |

The online timeline is the default graph — "when was this node online"
was the explicit ask.

### Interaction

| Key | Action |
|---|---|
| `h`/`l`, `←`/`→` | pan left/right |
| `+`/`-` | zoom in/out (x axis) |
| `t` | cycle fetch range 1h → 6h → 24h → 7d (unchanged) |
| `r` | refetch current window |
| `H`/`L` or `shift+←/→` | jump to start / end |
| mouse wheel / drag | pan; ctrl+wheel zoom |
| `v` (from detail) | open graphs, cycle metric |

Zoom acts on the fetched series first (no refetch while the window stays
inside the fetched range). When a zoom/pan would leave the fetched
range, fetch the new window from Influx (`EngineCommand::FetchGraph`
gains optional start/end). Fetches stay generation-guarded like today:
stale replies are dropped by (key, metric, range/window) match.

### Time axis

- X axis labeled with wall-clock times (`HH:MM`, `Mon HH:MM` for 7d).
  This was the "really important" item; F5 in the M10 fixpack lands the
  basic labels, iteration 2 keeps them correct under pan/zoom.
- Grid lines at nice time boundaries (hour/6h/day depending on span).

### Implementation sketch

- `tui/graphs.rs` grows a `GraphView` state: `{ metric, window: (t0, t1),
  series, state }` where the view owns pan/zoom math as pure functions.
- Renderer maps window → chart bounds; axis label generation is a pure
  `window -> Vec<(position, label)>`.
- The online timeline is a custom widget (stepped blocks), not a Chart
  series — Chart lines don't read as a band.

## 2. Virtualized node list

Target: 100k nodes, sub-10 ms frames.

- The projection (filter + sort → ordered node keys) moves out of the
  render path into `AppState`, cached and versioned:
  - store mutation (RTS delta) bumps a `store_version`;
  - filter/sort change bumps a `projection_version`;
  - the reducer recomputes the projection only when either version
    moved since the cached run.
- Incremental update on RTS deltas is a later refinement: start with
  recompute-on-mutation, which is already fast enough for a sorted-key
  pass over 100k entries (~ms), and only the cache lookup runs per
  frame.
- Rendering: `Table` gets only the visible slice plus overscan rows
  (e.g. viewport height + 10). Selection index, not scroll offset, is
  the source of truth; scrolling past the end is a no-op (fixes the
  same class of bug as F3 at the list level).
- `ToBottom` (`G`) jumps to the last row by index — no dependence on
  rendered rows.
- Node-count line shows `visible/total after filter` — with huge lists
  the filtered count matters more than raw totals.
- Sort performance: keep comparators allocation-free (precomputed sort
  keys where cheap); avoid per-frame `format!` in row building for rows
  outside the window (they don't exist anymore by construction).

### Testing

- Projection cache: mutate store / change filter → assert recompute
  happened or didn't, by version counters.
- Window math: pure function tests — index, height, overscan → slice
  bounds, at both edges (top, bottom, empty list, list shorter than
  viewport).
- Render test with a 100k-node synthetic store asserting frame time is
  bounded by construction (window size), via `TestBackend` + visible-row
  assertions, not 100k row rendering.
- Snapshot tests unchanged (small fixtures still render identically).

## 3. Wireshark-feel filter and sort system

Wireshark's display filter is the model: a typed expression language
over known fields with syntax highlighting, inline error reporting, and
field/value completion. We already have the grammar core from
iteration 1 (`key:value`, comparisons, and/or/not, parens).

### The feel, concretely

| Wireshark trait | wntui equivalent |
|---|---|
| Field reference (man page) | `:help fields` overlay + `?` on the filter line: every field, its type, comparators, examples |
| Typing errors shown as you type | parse errors rendered inline under the filter line with caret position, live |
| Green/red validity indicator | filter line shows `✓ valid` / `✗ error` (ASCII: `[ok]` / `[err]`) and the live match count only when valid |
| Autocomplete (`Tab`) | complete field names, operators, and enum values (`status:` → online/uncertain/offline); fuzzy-ranked like the command palette |
| Apply vs. type | filter applies live (current behavior) — Wireshark's "Apply" button maps to `Enter`, clear maps to `Esc` |
| Sort by column | `s` still cycles, but the column header shows the active sort with direction; sort from the palette too (`:sort fw desc`) |
| Save/load filters | named filter profiles in config (`:filter-save`, `:filter-load`) — the small persistent piece that makes a filter language usable |

### UX details

- Filter line stays one line: expression + live count
  (`status:offline and fw:<5.2 — 12/148`), with the error state
  replacing the count while invalid.
- The `:help fields` reference is generated from the same field table
  that drives the parser, so it can't drift.
- Fuzzy free-text still works; it's documented as sugar for
  `name~text or addr~text or desc~text`.
- Suggestions and validity live in the input layer (pure functions
  over the cursor + expression), tested table-driven like the parser.

## Milestones

| # | Deliverable |
|---|---|
| N1 | Filter system revamp: error/validity states, `:help fields` overlay, Tab completion, sort direction in header, named filter profiles |
| N2 | Virtualized list: projection cache in AppState, windowed render, window-math tests, 100k smoke fixture |
| N3 | Graph time axis + WNT-style online timeline widget (band rendering, status colors) |
| N4 | Graph pan/zoom: windowed fetches, generation-guarded refetch, zoom-within-fetched-range fast path |

N1 and N2 are independent; N3 and N4 build on the graphs work from
iteration 1. Order in the table is the suggested sequence (filters
first: they define the field table N3's timeline tooltip and N2's
count line both reuse).

## Testing and gates

Same rules as iteration 1: `cargo fmt`, `cargo clippy --all-targets
-- -D warnings`, `cargo test --workspace` per milestone; pure functions
tested table-driven; `TestBackend` for rendering; insta snapshots for
page composition. New snapshots will need `cargo insta review` when
the filter line and graphs pages change.
