# WNT Protocol Buffers files

Copied verbatim from github.com/wirepas/backend-apis
(`wnt/protocol_buffers_files/`), Apache License 2.0. See LICENSE in that
repository for the full text.

| File | Purpose |
|---|---|
| commons.proto | BaseRole, StackProfile, ScratchpadAction, AppConfigData, FullRole |
| internal.proto | OnlineStatus, NodeMetadata, Network, TraveltimeKPI, SinkPseudoIdMap, FloorPlan, Area |
| message.proto | root Message / MessageCollection, DiagnosticsData, RxData, ScratchpadStatus, NodeMessage, GetScratchpadStatus |
| otap.proto | OTAPState, SetOTAPState(Response), OTAPMethod, OTAPNPADDelay |
| remote_api.proto | RemoteApiRequest(Collection), RemoteApiResponse, BaseRole changes |
| positioning.proto | WPE MeshData / Status / Query (Message fields 130–131) |
| nanopb.proto | nanopb field options (build-time; embedded targets) |

## Regenerating bindings (Rust)

`message.proto` imports the others. For prost-build, either ship
`nanopb.proto` + `google/protobuf/descriptor.proto` (nanopb.proto imports
descriptor.proto) or strip the `[(nanopb)...]` options before compiling —
prost ignores nanopb options once the extensions are resolvable.

Field numbers in these files are the contract with the Influx column names
(`Message_<n>_<m>`). Do not renumber.
