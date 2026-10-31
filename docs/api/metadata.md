# Metadata service (WS 8812, JSON)

Request/response service. Every request carries `session_id` from login.
Updates made here fan out to all clients (including this one) via the RTS
stream as `metadata_update_message` or `node_metadata`.

Envelope:

```
request:  { "data": { ... }, "session_id": "<id>", "type": <n>, "version": 5 }
response: { "data": { ... }, "result": 1, "type": <n>, "version": 5 }
```

All mutating messages accept `originator_token` in `data`.

## Get networks (1041)

Returns id/name mappings **only for networks explicitly named via the API**.
Auto-created networks (seen traffic but never named) are absent. To list
all networks, derive the set from `network_id` values in the RTS stream and
diff against this response.

Request: `{ "data": {}, "session_id": "...", "type": 1041, "version": 5 }`

```json
{
  "data": { "networks": [ { "id": 42, "name": "Plant floor" } ] },
  "result": 1, "type": 1041, "version": 5
}
```

## Network metadata (1042–1044)

- Create (1042): data.networks[] { id, name } → result only.
- Update (1043): rename. data.networks[] { id, name }.
- Delete (1044): data.networks[] { id, is_delete_nodes }.
  `is_delete_nodes: true` deletes all nodes of the network too.

## Set node metadata (1061)

The one call for editable node attributes. Per node: `id` + `network_id`
mandatory, all others optional (send only changes). Supports batch (nodes[]).

| Field | Type | Meaning |
|---|---|---|
| id | uint32 | node address |
| network_id | uint32 | network |
| name | string | display name |
| description | string | free text |
| is_approved | bool | approve node for display |
| is_virtual | bool | planning node |
| is_anchor | bool | positioning role (anchor/tag) |
| latitude / longitude / altitude | double | WGS84 position |
| rssi_offset | int32 [-127..127] | anchor RSSI calibration |

```json
{
  "data": {
    "nodes": [ { "id": 123, "network_id": 42, "name": "Line 3 sensor" } ],
    "originator_token": "<uuid>"
  },
  "session_id": "...", "type": 1061, "version": 5
}
```

Echoed back through RTS as `node_metadata`.

## Change node id / network id (1062)

Moves a node between networks or re-addresses it. Part of the migration
workflow together with Remote API (network address/channel/keys).

## Delete node (1063)

Removes real-time situation + metadata. Node reappears if it sends data
again. Batch capable: data.nodes[] { id, network_id }.

## Set network persistent data (1071) — NPD / app config

Sets diagnostics interval and application config data (hex string).
Global per network or per-sink via `sink_node_ids`.

```json
{
  "data": {
    "networks": [ {
      "id": 42,
      "application_data": "112233445566778899AABBCCDDEEFF",
      "diagnostics_interval": 60,
      "is_override_on": false,
      "sink_node_ids": [1, 10]
    } ],
    "originator_token": "<uuid>"
  },
  "session_id": "...", "type": 1071, "version": 5
}
```

`is_override_on: true` → WNT re-applies NPD if changed outside WNT.
Results arrive per sink via RTS `app_config` + `app_config_response`
(error codes like INVALID_APP_CONFIG, SINK_TIMEOUT — see message.proto
AppConfigResponse).

## Get scratchpad status (1074)

Polls scratchpad/OTAP state per node in a network. Results arrive via RTS
`scratchpad_status` per node — not in the response.

```json
{
  "data": {
    "networks": [ {
      "id": 42,
      "resend_interval_s": 180,
      "is_close": false,
      "is_sink_only": false
    } ]
  },
  "session_id": "...", "type": 1074, "version": 5
}
```

| resend_interval_s | is_close | Meaning |
|---|---|---|
| 0 | false | single shot |
| >0 | false | start continuous query |
| 0 | true | stop continuous query |

## Send Remote API request (1072)

Node attribute changes at runtime: role, low-latency flag, autorole,
network address/channel, cipher/auth keys, node address. Broadcast or
unicast, optional scheduled start/activation/cancellation. Full message
shapes in remote-api-otap.md.

## Get components information (1081)

Triggers RTS `backend_component_info` + `gateway_info` messages (backend
health, gateway model/version). Use for a "system status" view.

## Buildings / floor plans / areas (1001–1034)

Map/floor-plan related; irrelevant for a mapless TUI. Listed only for
completeness (the TUI will not implement them in iteration 1).
