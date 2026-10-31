# wntui

A terminal client for a Wirepas WNT v4 backend. The TUI is the primary
interface; a CLI provides scriptable equivalents. Vim-modal feel, purple
theme, node status colors matching the official client.

Feature set (iteration 1):

- authenticate against the WNT backend (auth WS), reconnect + relogin
- live node list over the RTS stream, colored by online status
- fuzzy + predicate filtering (`/`), sorting, network picker
- node detail view (identity, mesh, power, firmware, positioning)
- edit node name/description (admin sessions only, metadata 1061)
- history graphs from Influx (voltage, travel time, online, boots, packets)
- CLI subcommands mirroring the read actions

## Build

Requires a recent Rust toolchain (edition 2024). Everything else —
protobuf codegen, TLS — is handled by Cargo:

```
cargo build --release
```

`crates/core` compiles `docs/api/protos/message.proto` at build time via
prost/protox; no protoc install is needed.

Quality gates used by the project:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

## Configure

`wntui` reads `$XDG_CONFIG_HOME/wntui/config.toml` (typically
`~/.config/wntui/config.toml`):

```toml
default_profile = "prod"

[profiles.prod]
host = "wnt.example.com"
username = "admin"
tls = true
verify_tls = true
ports = { auth = 8813, metadata = 8812, rts = 8811, influx = 8886 }
```

- `host` — backend hostname or IP
- `username` — login user; the password is never stored in the config: it
  comes from the OS keyring (`wntui auth login`), the `WNT_PASSWORD`
  environment variable, or an interactive prompt — in that order
- `tls` / `verify_tls` — connection scheme and certificate verification
  (`verify_tls` is accepted but not yet enforced: self-signed backends
  fail until it is wired to the TLS acceptor)
- `ports` — WNT service ports; defaults are shown

## Auth

`wntui auth login` prompts for the password, verifies it against the
backend's auth service, and stores it in the OS keyring (Secret Service
on Linux, Keychain on macOS, Credential Manager on Windows). After
that every launch — TUI and CLI — logs in without a prompt:

```
wntui auth login [--profile P]     # verify + store
wntui auth status [--profile P]    # where the password comes from
wntui auth logout [--profile P]    # remove the stored credential
```

Credentials are stored per profile and host (`profile/user@host`), so
the same profile name against two backends keeps two secrets. A missing
or locked keyring falls back to the `WNT_PASSWORD` environment variable
and then to an interactive prompt — nothing is ever written to the
config file.

## Run

```
wntui                    # TUI against the default profile
wntui --profile prod     # ... with an explicit profile
wntui --config FILE      # ... with an explicit config file
wntui --demo             # TUI with a synthetic network, no backend needed
```

## Install

```
cargo install --path crates/app
```

The binary lands in `~/.cargo/bin/wntui`. On hosts where cargo comes
from a nix profile, `~/.cargo/bin` is not on PATH by default — symlink
it somewhere that is:

```
ln -s ~/.cargo/bin/wntui ~/.local/bin/wntui
```

In the TUI: `?` shows the keymap for the current page, `:` opens the
command palette, `/` filters the node list, `j/k` move, `enter` opens a
node's detail. In the detail, the cursor walks the fields line by line:
editable fields (blue) open the editor on enter, graphable fields
(purple) open their graph; `t` cycles the graph time range. Editing
name/description requires an admin session.

Graphs carry a wall-clock time axis: grid lines at nice boundaries
(15m/hour/6h/day depending on the span) with `HH:MM` labels aligned under
them. The online status graph is a stepped band — green in the top lane
while the node was online, red in the bottom lane while offline, yellow
in between for uncertain — so "when was this node online" reads directly
off the screen.

On the graphs page the view is explorable: `h`/`l` (or the arrows, or
the mouse wheel) pan, `+`/`-` (or ctrl+wheel) zoom, `H`/`L` jump to the
start/end, `r` refetches. Zooming in never refetches — it acts on the
fetched series; panning or zooming past the fetched bounds fetches that
window from Influx, and the axis re-labels itself to the new span.

## Filtering and sorting

`/` opens the filter editor; the same expression works for
`wntui nodes --filter`. Free text is fuzzy-matched against names;
`field:value` predicates combine with `and`, `or`, `not`, and parens.

```
conveyor                        # fuzzy: any node whose name fuzzies to it
status:offline                  # online, offline, uncertain (on/off/unc work)
role:sub and voltage:>3         # role: sink, head, sub, unknown
voltage:<3.0 or voltage:>3.2
fw:5.2.1.0                      # exact firmware, or fw:>5.1.9.9
fw:>5.1 and not role:sink
age:>1h                         # not seen for over an hour (s/m/h/d/w)
net:42 or addr:3736             # addr and net take decimal or 0x hex
addr:>0x2 and addr:<0x9         # ranges compose with and
boots:>10
approved:no
(status:offline or status:uncertain) and age:>30m
```

Sorting: `s` cycles the sort field, `S` flips the direction, or jump
straight to one with the palette: `:sort status desc`, `:sort name asc`,
`:sort addr`. Fields: addr, name, status, role, voltage, fw, age. The
active sort shows as `▲`/`▼` on its column header.

The list stays responsive on production-size networks: the filter+sort
projection is cached and recomputed only when the store, the filter, or
the sort changes (age filters refresh once per second), and only the
visible rows plus a small overscan are rendered each frame — 100k nodes
render in bounded time.

### Live feedback in the filter editor

While typing, the filter line shows `[ok]` or `[err]` with the message
and a caret under the offending token — Wireshark-style. `tab` completes
field names (`vol` → `voltage`), offers the comparators (`voltage` →
`voltage:`), and completes enum values (`status:o` → `status:online`,
with the remaining choices shown). `:fields` opens the field reference
overlay: every field, its comparators, and a working example, generated
from the parser's own field table.

### Saved filters

Named filter profiles persist in the config file:

```
/filter, type a filter, enter
:filter-save offline-lab     # saves the active filter under a name
:filter-load offline-lab     # applies it again (session or restart)
:filters                     # lists the saved names
```

## CLI

Read-only subcommands share the config and password resolution with the
TUI and print JSON when given `--json`:

```
wntui auth login|logout|status   # OS-keyring credential management
wntui networks [--profile P] [--json]      # list networks seen in the live state
wntui nodes [--network N] [--filter EXPR] [--json]
wntui node <address> [--network N] [--json]  # address: decimal or 0x-prefixed hex
wntui probe [--profile P]                 # dump Influx measurements + field keys
wntui graph <metric> <address> [--network N] [--range 24h] [--json]
                                         # metric: voltage, traveltime,
                                         #   traveltime-qos1, online, boots, packets
```

`networks`, `nodes` and `node` log in, wait for the live node snapshot,
then project the same store the TUI renders. `--demo` swaps in the demo
backend: `wntui nodes --demo` works with no backend at all. `--filter`
takes the same expression as the TUI's `/` filter (`status:offline and
voltage:<3.0`, free text is fuzzy).

`probe` runs `SHOW MEASUREMENTS` and `SHOW FIELD KEYS ON wirepas` against
the backend's Influx and dumps the results — run it once against a real
backend to pin the actual column names before trusting graph data.

The Influx auth branch is chosen at login: if the login response carries
an `influx_token` (WNT >= 4.4) the client uses token auth; otherwise it
falls back to basic auth with the login credentials (Influx v1).

## Layout

```
crates/core    wntui-core: wire protocols (auth, metadata, RTS, Influx),
               node store, session supervision — no TUI deps
crates/app     wntui: TUI + CLI binary
docs/api       protocol notes: protos/, auth, metadata, time-series (Influx)
docs/plan      iteration plans
```
