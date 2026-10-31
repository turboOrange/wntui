# WNT backend API — overview

Wirepas Network Tool (WNT) backend v4.x. Documented here from the official
"WNT4 backend API" doc v4.12 (2024-01-04) plus the public protobuf
definitions in `wirepas/backend-apis`.

## EOS note

Wirepas has ended support for WNT/WPE. Existing deployments should migrate
to Wirepas NMS (REST + MQTT APIs). WNT remains functional; this tool targets
WNT backend v4.x.

## Endpoints

| Component | Protocol | Port | Encoding | Auth |
|---|---|---|---|---|
| Authentication | WebSocket | 8813 | JSON | username/password |
| Metadata | WebSocket | 8812 | JSON | session_id |
| Real time situation (RTS) | WebSocket | 8811 | JSON handshake, then Protocol Buffers | session_id |
| Time series (Influx) | HTTP GET | 8886 | JSON (InfluxDB) | WNT <4.4: user/pass; >=4.4: token from login |
| MQTT JSON API | MQTT over TCP / WS | 8883 / 9002 | JSON | dedicated mqttjsonuser profile |

Default ports; all TCP. Client must be able to reach 8811, 8812, 8813, 8886.

## Protocol version

Message field `version`. Version 5 = backend 4.x. Backend rejects other
versions. Every JSON message carries `version: 5`.

| version | backend |
|---|---|
| 2 | 1.6, 1.7 |
| 3 | 2.0 |
| 4 | 3.0 |
| 5 | 4.x |

## Roles

| Role | Number | Rights |
|---|---|---|
| Administrator | 1 | query + mutate |
| Operator | 2 | query only |

Role is returned by login; every mutating message requires admin.

## JSON message envelope

All auth + metadata messages:

```
request:  { "data": { ... }, "session_id": "<id>", "type": <n>, "version": 5 }
response: { "data": { ... }, "result": <code>, "type": <n>, "version": 5 }
```

- `type` selects the message (see per-service tables).
- `result`: 1 = ok, other = error (codes per service).
- Login has no session_id in the request.
- `originator_token` (client-chosen UUID string) is echoed through the RTS
  stream so a client can recognize its own changes; RTS updates may arrive
  before the request response.

## High-level data flow

```
                    +------------------+
   gateways -MQTT-> |  WNT backend     |
                    |                  |   WebSocket 8813 JSON
   WPE       -MQTT->|  auth / metadata | <----------------  TUI (this project)
                    |  RTS / Influx    |   WebSocket 8812 JSON
                    |                  | <----------------
                    +------------------+   WebSocket 8811 protobuf
                                          HTTP 8886 InfluxQL
                                          MQTT 8883 (optional)
```

1. Connect WS 8813, login → `session_id` (+ `influx_token` on >=4.4).
2. Connect WS 8812 (metadata) with session_id; request/response JSON.
3. Connect WS 8811 (RTS) with session_id; receive full node state, then deltas.
4. Query Influx (8886) for history/graphs. Do not poll constantly.

## Metadata message types (WS 8812)

| type | Message | Role |
|---|---|---|
| 1001 | Get buildings | all |
| 1002–1004 | Create/update/delete building | admin |
| 1011 | Get building's floor plans | all |
| 1012–1014 | Create/update/delete floor plan | admin |
| 1021 | Get floor plan image data | all |
| 1022 | Set floor plan image data | admin |
| 1031 | Get areas | all |
| 1032–1034 | Create/update/delete area | admin |
| 1041 | Get networks | all |
| 1042–1044 | Create/update/delete network metadata | admin |
| 1051 | Add node to floor plan | admin |
| 1052 | Remove node from floor plan | admin |
| 1061 | Set node metadata | admin |
| 1062 | Change node id and/or network id | admin |
| 1063 | Delete node | admin |
| 1071 | Set network persistent data | admin |
| 1072 | Send Remote API request | admin |
| 1074 | Get scratchpad status | admin |
| 1081 | Get components information | all |

Auth message types (WS 8813): 1 login, 11 get users, 12 create user,
13 update user, 14 delete user.

Result codes:

- auth: 1 ok, 2 generic, 3 invalid credentials, 4 wrong protocol version,
  5 no rights, 6 invalid user id, 7 user exists, 8 invalid message,
  9 invalid session id
- metadata: 1 ok, 2 generic, 4 wrong protocol version, 5 no rights,
  6 invalid id, 8 invalid message, 9 invalid session id

## Real time situation (WS 8811)

Protobuf stream of `MessageCollection` (root `Message` items, message.proto).
After JSON auth handshake: full snapshot, then deltas. Node identity key:
`(network_id, source_address)`. Details in realtime-situation.md.

## Time series (Influx, HTTP 8886)

Direct InfluxDB access, database `wirepas`. Measurements: analytics_boot,
analytics_next_hop, analytics_nodestate, analytics_packet,
analytics_traveltime_kpi, location_measurement, location_update,
node_metadata, remote_api_response, endpoint_<id>. Details in time-series.md.

## Protobuf definitions

`protos/` contains the official files (Apache-2.0):

- `message.proto` — root Message/MessageCollection, diagnostics, RxData,
  scratchpad status, node downlink messages
- `commons.proto` — BaseRole, ScratchpadAction, StackProfile, AppConfigData
- `internal.proto` — OnlineStatus, NodeMetadata, Network, SinkPseudoIdMap, TraveltimeKPI
- `otap.proto` — SetOTAPState / SetOTAPStateResponse
- `remote_api.proto` — RemoteApiRequest(Collection)/Response
- `positioning.proto` — WPE MeshData/Status (fields 130–131 of Message)
- `nanopb.proto` — nanopb field options (build-time only)

Regenerate language bindings from these for decoding the RTS stream.

## Gateway ↔ backend

Gateway-to-backend API is separate (WP-RM-128), also in wirepas/backend-apis
(`gateway_to_backend/`). Not needed for this client — the TUI talks to the
backend, not the gateways.
