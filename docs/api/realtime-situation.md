# Real time situation service (WS 8811, protobuf)

The live node-state stream. This is the heart of the TUI: full node state
on connect, deltas afterwards.

## Handshake

1. Open WS 8811.
2. Send JSON: `{ "session_id": "<id>", "version": 5 }`
3. Receive JSON: `{ "result": 1, "version": 5 }`
4. Service switches to write-only mode: all further frames are
   Protocol Buffers (`MessageCollection`, message.proto). Client never
   writes again on this socket.

## Message framing

Root item is `Message`; wire container is
`MessageCollection { repeated Message message_collection = 1 }`.
(Framing between successive collections — length prefix per WS binary
message — matches how the WNT client consumes it; verify on first connect
against a live backend.)

## Initial full snapshot

First messages are `rtsituation_metadata` (one per RTS cluster cell):

```
Message -> rtsituation_metadata { cluster_no, cluster_size, node_count }
```

Sum `node_count` over all cells = total node count to expect. When the
number of distinct `(network_id, source_address)` pairs received equals
the sum, the initial state is complete. Render as "loading N/M nodes".

## Delta updates

After the snapshot, only changes are sent (diff vs backend's last known
situation). Processing cadence: control operations every 5 s, network
traffic batched every 30 s; a single Message may merge several raw
messages of one node. Always apply deltas onto the stored node state.

## Node identity

Key: `(network_id, source_address)`. Node address is uint32 (12-bit
in the mesh). No stable UUID — node id/network can change via message 1062.

## Message fields relevant to the TUI

All fields of root `Message` (message.proto):

| Field # | Name | Content |
|---|---|---|
| 1 | id | packet id list |
| 2 | network_id | |
| 3 | gateway_id | string |
| 4 | tx_time | node generation time, epoch ms |
| 5 | rx_time | gateway receive time, epoch ms |
| 7 | source_address | node id |
| 9 | travel_time_ms | rx_time - tx_time |
| 50 | diagnostics | DiagnosticsData — role, voltage, FW/HW, events, neighbors, ... |
| 51 | rx_data | RxData — app payload uplink (endpoints, qos, payload) |
| 52 | app_config | AppConfigData (sinks only): NPD, diagnostics interval |
| 53 | stack_profile | StackProfile enum |
| 54 | measurement | device measurements |
| 55 | gw_heartbeat | gateway heartbeat |
| 56 | tx_data | downlink data echoed |
| 58 | network_channel | sinks only |
| 59 | security_enabled | sinks only |
| 61 | app_config_response | result of NPD set (sinks only) |
| 63 | gateway_info | model, version, api_version |
| 64 | backend_component_info | |
| 65 | scratchpad_status | ScratchpadStatus — OTAP state per node |
| 71 | online_status | OnlineStatus — see below |
| 72 | traveltime_kpi | TraveltimeKPI — min/avg/max per QoS |
| 73 | node_metadata | NodeMetadata — name, description, position, approval |
| 74 | rtsituation_metadata | initial node counts |
| 76 | metadata_update_message | non-node metadata deltas (networks, users, ...) |
| 77 | sink_pseudo_id_map | sink ↔ gateway mapping |
| 80 | remoteapi_requests | in-flight remote API request collection |
| 81 | remoteapi_response | RemoteApiResponse |
| 90 | node_messages | downlink NodeMessage (to network) |
| 91 | get_scratchpad_status | |
| 100 | set_otap_state | OTAP control |
| 101 | set_otap_state_response | |
| 102 | set_scratchpad_action | |
| 130/131 | positioning_mesh_data / positioning_status_data | WPE |
| 160 | calculated_values_from_diagnostics | derived metrics (e.g. LE TX load %) |

## OnlineStatus (internal.proto)

```
OFFLINE = 0, UNCERTAIN = 1, ONLINE = 2
update_time, last_time_series_write, previous_status,
is_sink_online_in_gateway (sinks)
```

Client-side coloring (matches official WNT client):

| Status | Color | Meaning |
|---|---|---|
| ONLINE (2) | green | seen recently |
| UNCERTAIN (1) | yellow | might be offline |
| OFFLINE (0) | red | offline |
| field absent | red | treat as offline (documented behavior after backend restart) |

## DiagnosticsData highlights (message.proto field 50)

- Role: `role` (SUBNODE/HEADNODE/SINK), `cb_mac` (true=low latency),
  `is_autorole`
- Power: `voltage` (V), `duty_cycle` %, `tx_powers[]`, `rx_gain`
- Versions: `sw_version_{major,minor,maintenance,devel}`,
  `application_version_*`, `hw_magic` (chip enum), `stack_profile`
- OTAP: `otap_enabled`, `scratchpad_sequence`, `scratchpad_firmware_sequence`,
  `scratchpad_transfers`
- Health: `boot_count`, `boot_reason`, `mem_alloc_fails`,
  `max/average_buffer_usage`, `events[]` (scan/role/reset reasons incl.
  RESET_WATCHDOG, RESET_BROWNOUT, ...)
- Mesh: `neighbors[]` (address, rssi_dBm, neighbor_type: next-hop /
  alternative route / member), `cost[]` per QoS, `sink_address`,
  `routers_in_neighborhood`, blacklisted channels,
  `installation_quality` + `installation_quality_errors`
  (NO_ROUTE_TO_SINK / NOT_ENOUGH_NEIGHBORS / BAD_RSSI)
- Radio: `cluster_channel_MHz`, `network_channel_packet_error_rate_percent`,
  `next_hop_rssi_dBm`, `next_hop_radio_power_dB`, `cb_mac_load`

## ScratchpadStatus (message.proto field 65)

- stored: `stored_scratchpad_{size,crc,sequence,type,status}`
- processed firmware: `processed_fw_*` (version major/minor/maintenance/devel,
  area_id, sequence, crc, size)
- processed application: `processed_application_*` (same shape)
- pending OTAP: `scratchpad_action`, `target_sequence`, `target_crc`,
  `target_delay_minutes`, `remaining_time_minutes`

## metadata_update_message

Delta of non-node metadata: added_or_changed/deleted arrays for networks,
users, buildings, floor plans, areas. Network renames arrive here.

## TraveltimeKPI (field 72)

`qos0_{minimum,average,maximum}`, `qos1_{minimum,average,maximum}`,
`window_duration` (s). Per-node; good sparkline candidate from live stream
(with history from Influx analytics_traveltime_kpi).

## Client state model (planned)

```
NodeKey = (network_id: u32, source_address: u32)

NodeState {
  online_status, diagnostics (subset), node_metadata (subset),
  app_config?, scratchpad_status?, traveltime_kpi?,
  rx_time, gateway_id
}
```

- Keep a HashMap<NodeKey, NodeState>, apply snapshot then deltas.
- Track `expected_node_count` from rtsituation_metadata for progress UI.
- Filter/sort run over this map; UI renders a projection (Vec of refs).
- Non-sink nodes may lack online_status after backend restart → offline.
