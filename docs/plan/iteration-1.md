# Iteration 1 — plan

A terminal client for a Wirepas WNT v4 backend. The TUI is the primary
interface; a CLI provides scriptable equivalents. Vim-modal feel, glow-like
reading comfort, purple theme, node status colors matching the official UI.

## Goal

An operator can launch `wntui`, authenticate, pick a network, see a live
list of nodes colored by online status, filter that list, inspect a node,
view its history graphs, and edit its name/description — entirely in the
terminal. CLI subcommands expose the same read/limited-write actions.

## In scope (iteration 1)

1. Authentication (WNT backend v4.x) — see "Authentication" below.
2. Network listing and switching.
- Node list: live via RTS, colored by online status, sortable.
- Filter system: fuzzy text + field predicates, live.
- Node detail: identity, status, radio/diagnostics, editions.
- Edit node metadata: name, description (+ other editable fields we support).
- History graphs via Influx: voltage, travel time, online/boot timeline.
- CLI subcommands mirroring read + edit operations.
- Purple theme; vim + glow keybindings.

## Out of scope (later iterations)

- Floor plans / map (explicitly dropped).
- OTAP upload/activation (needs the missing metadata type verified).
- Remote API / network migrations (re-address, channel/key changes).
- Downlink messages.
- MQTT JSON API.
- User management, buildings, areas.
- Write-ahead/offline mode.

## Feature-to-API map

| Feature | API |
|---|---|
| Login | Auth WS 8813, type 1 |
| List networks | Metadata 1041 + networks discovered from RTS |
| Switch network | client-side selection over node store |
| Node list / live state | RTS WS 8811 (protobuf snapshot + deltas) |
| Node detail | node store (diagnostics, metadata, scratchpad) |
| Edit name/description/attrs | Metadata 1061 (admin) |
| Node history graphs | Influx HTTP 8886 (InfluxQL) |
| Node status colors | `OnlineStatus.status`: 2 online, 1 uncertain, 0 offline |

## User experience

### Keybindings (vim + glow)

Normal mode:

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | move selection |
| `g`/`G` | top / bottom |
| `ctrl-d`/`ctrl-u` | half page |
| `Enter` | open node detail |
| `Esc`/`q` | back / close overlay / quit from list |
| `/` | fuzzy filter (insert mode) |
| `:` | command palette |
| `n`/`net` via `:net` | switch network |
| `s` | cycle sort column |
| `r` | refresh / re-sync full state |
| `a` | toggle approved filter (quick view) |
| `e` | edit selected node (detail view) |
| `?` | help overlay |
| mouse wheel | scroll |
| `ctrl-c` | quit |

Command palette (`:`) — vim-style, fuzzy-completed:
`:net <id|name>`, `:filter <expr>`, `:sort <field>`, `:graph <metric>`,
`:edit`, `:profile <name>`, `:connect`, `:probe`, `:q`.

Node detail: `j/k` scroll, `g/G`, `ctrl-d/u`, `e` edit, `:` palette, `q` back.

### Theme (purple)

Defined in `ui/theme.rs` as named constants. 24-bit RGB, degrades to the
16-color palette when the terminal lacks truecolor.

| Token | RGB | Use |
|---|---|---|
| primary | 178, 132, 255 | titles, active border, focus |
| accent | 147, 112, 219 | highlights, key hints |
| dim | 120, 90, 180 | inactive borders, labels |
| subtle bg | 30, 22, 44 | selection background |
| status_online | 80, 200, 120 | green — OnlineStatus 2 |
| status_uncertain | 230, 180, 60 | yellow — OnlineStatus 1 |
| status_offline | 220, 70, 70 | offline / missing status |
| text | default fg | body |
| muted | dark gray | secondary text |

Status colors match the official WNT client. `--no-color` / `NO_COLOR`
falls back to a monochrome-safe rendering (status shown as symbol + text).

### Layout

```
+--------------------------------------------------------------+
| wntui  network: Plant floor (42)   profile: prod   admin     |  header
+--------------------------------------------------------------+
| filter: status:offline and fw:<5.2          12/148 nodes     |  filter line
+--------------------------------------------------------------+
|   STATUS  ADDR    NAME              ROLE    V      FW    AGE  |  column hdr
| ● online  0x1A20  Line 3 sensor     sub    3.21   5.2.0  2s   |
| ◐ uncert  0x1A44  Gate west         head   3.18   5.2.0  1m   |
| ○ offline 0x1B02  Line 7 sensor     sub    2.9    4.8.1  3h   |
+--------------------------------------------------------------+
| NORMAL  j/k move  enter detail  / filter  : cmd  ? help      |  status bar
+--------------------------------------------------------------+
```

Status glyphs are ASCII-safe fallbacks for `●◐○`.

## Architecture

Elm/TEA style: a single pure reducer, effects as data.

```
terminal events ─┐
ws/influx msgs ──┼─> Msg ──> update(state, msg) -> (state, Vec<Effect>)
clock ticks ─────┘                     │
                                       v
                              Effect runner (tokio tasks)
                                       │
                               view(state) -> Frame (ratatui)
```

- No shared mutable state across threads; each background task owns its
  socket and sends `Msg` into one `mpsc` channel.
- `update` and `view` are pure functions over `AppState`.
- Store: `HashMap<NodeKey, Node>`; apply full snapshot then protobuf deltas.
- Rendering is a pure projection: filter + sort -> `Vec<&Node>`.

### Crates

| Crate | Use |
|---|---|
| ratatui + crossterm | TUI rendering / input / mouse |
| tokio | async runtime |
| tokio-tungstenite | auth / metadata / RTS WebSockets |
| prost + prost-build | decode protobuf RTS stream |
| serde + serde_json | auth + metadata JSON messages |
| reqwest | Influx HTTP 8886 (rustls) |
| clap (derive) | CLI surface |
| nucleo-matcher | fuzzy filtering |
| toml + directories | config file |
| thiserror / anyhow | typed errors / CLI context |
| ratatui `Chart`/`Sparkline` | graphs (no extra chart dep) |

Dev / test only:

| Crate | Use |
|---|---|
| insta | snapshot tests for rendered pages and `--json` CLI output |
| assert_cmd + predicates | run the CLI binary and assert on output/exit |
| pretty_assertions | readable assertion diffs |
| tokio (test macros) | async transport tests |
| proptest (optional) | filter-parser property tests |

ratatui ships its own test harness (`TestBackend`) — no extra crate needed
for rendering tests.

Protobuf build: `build.rs` runs prost-build over `docs/api/protos/message.proto`
(nanopb options are ignored by prost; keep the files as-is for reference).

### Workspace layout

A Cargo workspace so the boundary is enforced mechanically: `core` cannot
import ratatui, and `app` cannot reach into a socket directly. The TUI
lives in one place divided by page; the API lives in another divided by
concern.

```
wntui/
├─ Cargo.toml                     workspace
├─ docs/api/protos/               protobuf sources (single source of truth)
└─ crates/
   ├─ core/                       wntui-core — headless engine, no TUI
   │  └─ src/
   │     ├─ lib.rs
   │     ├─ config/               profiles, load/save
   │     ├─ wire/                 transport, one module per concern
   │     │  ├─ mod.rs
   │     │  ├─ auth.rs            login (WS 8813)
   │     │  ├─ metadata.rs        request/response (WS 8812)
   │     │  ├─ realtime.rs        snapshot + delta stream (WS 8811)
   │     │  ├─ influx.rs          InfluxQL client (8886), v1/v2 auth
   │     │  └─ proto.rs           prost codegen include
   │     ├─ model/                domain types — Node, Network, status
   │     ├─ store/                NodeStore, delta merge
   │     ├─ session/              connection state machine, credentials
   │     ├─ filter/               predicate parser (pure)
   │     └─ engine/               commands in, events out, task supervision
   └─ app/                        wntui — binary: TUI + CLI
      └─ src/
         ├─ main.rs              dispatch: TUI vs CLI subcommand
         ├─ cli/                 clap defs + read-only handlers
         ├─ tui/
         │  ├─ app.rs            AppState, reducer wiring
         │  ├─ msg.rs            Msg (sum type)
         │  ├─ effect.rs         Effect (sum type)
         │  ├─ update.rs         pure reducer
         │  ├─ input/            keymap (Mode, Key) -> Action
         │  ├─ theme.rs
         │  ├─ components/       reusable widgets (table, kv, sparkline, modal)
         │  └─ pages/            one file per page
         │     ├─ nodes.rs
         │     ├─ node_detail.rs
         │     ├─ networks.rs
         │     ├─ graphs.rs
         │     ├─ edit.rs
         │     └─ help.rs
         └─ runtime/             event loop, effect runner, tick, cancellation
```

### Layering rules

- Dependency direction is one-way: `app → core`. Never the reverse.
- Prost/wire types stay inside `core::wire`; `model` is the only type the
  UI sees. Proto → model mapping happens in exactly one place.
- Each socket is an actor: a task owns it and communicates over channels.
  No `Arc<Mutex<Socket>>`.
- The TUI never calls the network. Pages emit `Action`, the reducer turns
  actions into `Effect`s, and the runtime executes effects against `core`.
  This single seam makes the CLI free and the UI testable.
- Connection lifecycle is a value:
  `Disconnected → Authenticating → Syncing → Live → Reconnecting`.
- Rendering is a pure projection; no I/O inside render functions.
- RTS input is chatty: process queued messages, render on a fixed tick
  (~50 ms), never per message.
- Leaving a page cancels its in-flight requests via a generation counter.
- Overlays are one `Overlay` enum (help / palette / prompt / confirm / edit),
  not a pile of booleans.
- Logs go to a file (`tracing` + file appender), never stdout — stdout
  would corrupt the TUI.
- Inject a `Clock` so "age"/timestamps are deterministic in tests.

## Authentication

### Confirmed facts

- Login over WS 8813: username + password -> `role` + `session_id`
  (+ `influx_token` on WNT >= 4.4).
- `session_id` authenticates the metadata (8812) and RTS (8811) sockets.
- Session dies when the auth socket closes; open sockets survive. So keep
  the auth socket alive for the process lifetime, and re-login on reconnect.
- Roles: admin (1) mutates, operator (2) read-only — UI must degrade cleanly.

### Resolved decisions

1. **Password**: hidden prompt on launch by default; `WNT_PASSWORD` env var
   used automatically when present (CLI/scripting). No plaintext storage.
   OS keyring is a later, optional enhancement.
2. **Multiple profiles**: named profiles in config; select with `--profile`
   or the `:profile` command.
3. **TLS**: support `ws`/`wss` and `http`/`https`; verify certs by default;
   `--insecure` to opt out (self-signed lab backends).
4. **Backend versions**: support both >= 4.4 and < 4.4. Login response tells
   the story: if `data.influx_token` is present → Influx v2 (use the token);
   otherwise → Influx v1 (reuse the login username/password). The Influx
   client takes an `InfluxCredentials` enum (`Token(String)` |
   `Basic{username, password}`) chosen at login time.

Config — `$XDG_CONFIG_HOME/wntui/config.toml`:

```toml
default_profile = "prod"

[profiles.prod]
host = "wnt.example.com"
username = "admin"
# password: never stored here; prompt / env (keyring later)
tls = true
verify_tls = true
ports = { auth = 8813, metadata = 8812, rts = 8811, influx = 8886 }
```

## Filter system

Small expression language; empty filter = all nodes.

- Free-text token → fuzzy match on name, address (hex/dec), description.
- Predicates: `key:value`, `key<value`, `key>value`, `!key:value`.
- Implicit AND between space-separated terms; `or`, `and`, `not`,
  parentheses for grouping.

Fields: `status` (online/uncertain/offline), `role` (sink/headnode/subnode),
`fw`, `app` (app version), `voltage` (v), `name`, `net`, `addr`, `approved`,
`anchor`, `virtual`, `boots` (boot_count), `age` (time since last seen).

Examples:

```
status:offline
role:sink or role:headnode
voltage:<3.0 and !status:online
line 3                      (fuzzy on free text)
```

Live match count in the filter line; `Esc` clears. Parser is a pure
`&str -> Result<Predicate, ParseError>`; `Predicate: Fn(&Node) -> bool`.

## Graphs (node detail)

Influx queries over 1h / 6h / 24h / 7d (range selector `t`):

| Metric | Measurement | Notes |
|---|---|---|
| Voltage | diagnostics (field `Message_50_7`) | verify measurement name |
| Travel time avg (QoS0/QoS1) | analytics_traveltime_kpi | line/area |
| Online timeline | analytics_nodestate | `Message_71_1` |
| Boot events | analytics_boot | count + boot_reason |
| Packet rate | analytics_packet | count per window |

Rendered with ratatui `Chart` (multi-series) / `Sparkline`. Query builder
keeps the field names in one constants module. Do not poll: fetch on
view-open and range change only.

Influx auth is chosen at login: `influx_token` present → v2 token auth;
absent → v1 basic auth with the login credentials. One `InfluxCredentials`
enum feeds the same query path either way.

First implementation step: `wntui probe` runs `SHOW MEASUREMENTS` and
`SHOW FIELD KEYS ON wirepas` and dumps them, so the real column names get
pinned before wiring the graphs.

## CLI surface

Iteration 1 CLI is **read-only** (writes land in a follow-up once the
write path is proven in the TUI). Editing is TUI-only for now.

```
wntui                              launch TUI
wntui networks [--profile P] [--json]
wntui nodes [--network N] [--filter EXPR] [--json]
wntui node <address> [--network N] [--json]
wntui graph <metric> <address> [--network N] [--range 24h] [--json]
wntui probe [--profile P]          dump Influx measurements + field keys
```

Global: `--profile`, `--host`, `--username`, `--insecure`, `--json`,
`--no-color`. Password resolution is shared with the TUI. `--json` output
makes the CLI scriptable (pairs with the same model layer).

Deferred to a follow-up (after the write path is proven in the TUI):

```
wntui rename <address> --name NEW [--network N]
wntui describe <address> --description TEXT [--network N]
```

## Editing attributes

Admin-only, via metadata 1061. Iteration 1 edits name and description
(the two the user named); `is_approved` and `rssi_offset` exposed if cheap.
Position (lat/lon/alt) and positioning role left to a later iteration
(needs validation UX; map dropped). Flow: edit in a modal text field,
confirm, send; authoritative value arrives back through RTS `node_metadata`
(use `originator_token` to flag own change). If role is operator, edit keys
are disabled with a status-bar hint.

## Code style

- Functional where it clarifies: pure functions, `AppState` as immutable
  data transformed in `update`, iterators/combinators over loops, sum types
  for status and events, `Result`/`Option` pipelines with `?`.
- Effects as data (`Effect` enum) executed by the runtime; no business
  logic inside task closures.
- Naming documents intent; no `///` doc comments (rely on names + this doc).
  Inline `//` comments only for protocol quirks (cite the docs file/field).
- Small modules, one concern each. `clippy` clean, `cargo fmt`.

## Testing strategy

The TUI is only hard to test when rendering, I/O, and state are tangled.
Here they are separate on purpose (pure `view`, pure reducer, effects as
data), so most of the app is tested with plain unit tests and the rest with
ratatui's built-in `TestBackend`. No real terminal, no real backend.

### What makes it testable (design rules)

- Render functions are pure: `fn render(state: &PageState, frame: &mut Frame)`
  and never do I/O.
- The reducer is pure: `update(state, msg) -> (State, Vec<Effect>)`. Effects
  are returned, not executed.
- `Clock` and transport are injected; no `SystemTime::now()` or sockets in
  render/update.
- Deterministic ordering: never render from a `HashMap` directly — project
  to a sorted `Vec` first. (Needed for stable snapshots.)

### The layers, cheapest first

1. **Pure logic tests** (`#[cfg(test)]` in each module, no terminal/async):
   - filter parser: `&str -> Predicate`, table-driven cases;
   - node store delta merge: feed proto `Message`s → assert `NodeStore`;
   - status → color/glyph mapping;
   - filter + sort projection: nodes + filter → ordered node ids;
   - keymap: `(Mode, Key) -> Action` table;
   - reducer navigation flows: drive sequences of `Msg::Key(..)` and assert
     the resulting state (open detail, back, open filter, apply, edit,
     confirm) — no I/O at all.
2. **Rendering tests with `TestBackend`** (ratatui's in-memory backend):

   ```rust
   let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
   terminal.draw(|frame| nodes::render(&state, frame)).unwrap();
   terminal.backend().assert_buffer_lines([ /* expected rows */ ]);
   ```

   - `assert_buffer_lines` for exact layout of small widgets/pages.
   - Cell inspection for color semantics: read `terminal.backend().buffer()[(x, y)].fg`
     and assert the status cell is green/yellow/red per `OnlineStatus`.
     This directly tests the "official UI colors" requirement.
   - Render every page at several widths (80 / 120 / 200) to catch
     truncation and overflow.
3. **Snapshot tests (`insta`)** for whole pages: stringify the buffer and
   `insta::assert_snapshot!(...)`, reviewed with `cargo insta review`.
   - Use for full-page appearance; prefer targeted assertions for things
     that should be stable (a node's name is present, status cell color)
     and snapshots for the overall composition.
   - Redact timestamps/IPs/ports that legitimately vary.
4. **Transport tests** (`core::wire`): clients sit behind traits, so tests
   feed a fake. Golden fixtures under `tests/fixtures/` hold real
   `MessageCollection` bytes (captured once, or synthesized with prost);
   assert bytes → `model`. One test runs a fake `tokio` WS server that
   sends captured frames to verify the reader's framing/reassembly.
5. **Influx tests**: query builder is pure (`metric + filters + range -> String`);
   parse fixture JSON responses from a recorded backend.
6. **CLI tests**: `assert_cmd` + `insta` for `--json` output and exit codes.
   Handlers call `core` directly, so they can also be unit-tested by calling
   the handler function and asserting the returned structs.
7. **Manual E2E via a replay mode**: a `--demo`/fixture event stream lets the
   TUI run with no backend — for hands-on visual checks, CI smoke
   (start → render a frame → quit), and docs/screenshots.

### Not tested on purpose

- ratatui widget internals (upstream's responsibility).
- Live backend behaviour — covered by the manual replay mode instead.

### Layout

- Unit tests co-located per module (`#[cfg(test)]`), matching the
  self-documenting / functional style.
- Integration tests + fixtures under `crates/core/tests/` and
  `crates/app/tests/`, golden bytes under `tests/fixtures/`.

## Milestones

| # | Deliverable |
|---|---|
| M0 | cargo scaffold, deps, protobuf codegen, config load, theme module |
| M1 | auth login + session, keep-alive, reconnect/relogin |
| M2 | RTS client: snapshot + deltas into node store; metadata Get networks |
| M3 | TUI shell: header/filter/status bar, help overlay, command palette, modal input, network picker + switch |
| M4 | node list: columns, status colors, sort, vim/glow navigation |
| M5 | filter engine: fuzzy + predicate parser, live count |
| M6 | node detail view |
| M7 | edit name/description (1061) in the TUI, operator/admin gating |
| M8 | `probe` + Influx graphs (voltage, traveltime, online, boots) with range select; Influx v1/v2 auth branch |
| M9 | CLI read-only subcommands (networks, nodes, node, graph, probe) + `--json` — done |
| M10 | polish: reconnect/error toasts, empty/loading states, README + review fixpack (below) — done |

Every milestone ends with `cargo fmt`, `cargo clippy -- -D warnings`, and the
tests for that milestone's layer (see Testing strategy). Pure logic (filter
parser, node store delta merge, status mapping, reducer, keymap) lands with
its milestone; `TestBackend` rendering tests land with each page (M3–M8);
`insta` snapshots and CLI `assert_cmd` tests land with M8–M9. The `--demo`
fixture harness lands with the TUI shell (M3) so later milestones can
smoke-test without a backend.

### M10 review fixpack

Feedback from the first live review, folded into M10:

| ID | Item | Status |
|---|---|---|
| F1 | Contextual help: `?` lists the keybindings of the current page (list, detail, graphs, networks), not one static popup | done |
| F2 | Network name moves to the header (as in the layout sketch above), out of the status bar | done |
| F3 | Node detail / graphs scroll bug: scrolling past the end accumulates offset in state (the clamp is view-only today), making scrolling back up slow — clamp to content height in the reducer | done (superseded by F6's cursor model) |
| F4 | Node list status column: the status word takes the status color too, not just the glyph | done |
| F5 | Graph x axis gets real time labels (HH:MM) — currently unlabeled | done |
| F6 | Node detail becomes field navigation: the cursor moves line by line (no page scrolling, no tab-switched edit menu). Editable fields carry a blue background, graph-viewable fields a purple background; `enter` edits / opens the graph of the field under the cursor | done |
| F7 | Network selection becomes a searchable dropdown: a text box filters the network list (name substring or id prefix), arrows move, enter selects | done |

The larger review items — interactive graphs (pan/zoom, WNT-style online
timeline), virtualized node list for huge networks, and Wireshark-style
filtering/sorting — are planned in `docs/plan/iteration-2.md`.

## Verify against a live backend (blockers)

1. RTS WebSocket framing: is each binary frame one serialized
   `MessageCollection`, or length-prefixed? Confirm by capturing the WNT
   client, then pin the reader.
2. Influx measurement/field names (`probe` command) especially where
   voltage/hop/traveltime actually live.
3. Whether the `Set OTAP state` metadata type exists (needed later, not now).
4. Which Influx version the target backend runs (detected from the login
   response at runtime; `probe` confirms the query path works).
5. TLS termination (direct wss vs reverse proxy) and certificate trust.

## Risks

- WNT EOS: keep the API layer behind traits so an NMS backend could be
  added later without touching UI/state.
- Protocol drift on the RTS stream: decode defensively; unknown fields are
  ignored by prost, so tolerate them.
