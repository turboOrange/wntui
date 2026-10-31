# wntui documentation

Research notes and plans for the Wirepas Network Tool TUI.

- [api/](api/) — Wirepas Network Tool (WNT) backend v4.x API reference
  - [overview](api/README.md) — architecture, endpoints, protocol versions
  - [authentication](api/authentication.md) — auth service, session, roles
  - [metadata](api/metadata.md) — request/response service (networks, nodes, NPD, scratchpad queries)
  - [realtime-situation](api/realtime-situation.md) — live node state stream (protobuf)
  - [time-series](api/time-series.md) — Influx diagnostics history (graphs)
  - [mqtt-json](api/mqtt-json.md) — MQTT JSON event feed
  - [remote-api-otap](api/remote-api-otap.md) — node configuration, migrations, firmware updates
  - [protos/](api/protos/) — protobuf definitions (Apache-2.0, from wirepas/backend-apis)
- [plan/](plan/) — implementation plans
  - [iteration-1](plan/iteration-1.md) — node list, filtering, node details, graphs, editing, CLI

## Sources

- WNT4 backend API doc v4.12 (developer.wirepas.com, article 77000499189)
- Wirepas Network Tool v4 Client User Guide v4.3 (article 77000499190)
- github.com/wirepas/backend-apis — wnt/protocol_buffers_files (Apache-2.0)

## Status caveat

WNT and WPE are end-of-life at Wirepas; new deployments use Wirepas NMS.
WNT backend v4.x is still fully functional and its API is public (this repo).
See `api/README.md` for the EOS note and NMS pointers.
