# Remote API, migrations, OTAP

The mutation paths: runtime node reconfiguration (migrations), firmware
updates (OTAP), and downlink. Message shapes from the protobuf files;
transport is metadata service type 1072 / 1074 (+ fields 80–81, 90–92,
100–102 on the RTS stream).

## Remote API (metadata type 1072)

Sends `RemoteApiRequestCollection` (remote_api.proto) to nodes. The
collection appears on RTS as `Message.remoteapi_requests` (field 80);
results come back as `Message.remoteapi_response` (field 81) and land in
Influx `remote_api_response`.

### RemoteApiRequest — per-node attribute changes

```proto
repeated uint32 addresses = 1;        // target nodes (broadcast mode)
optional uint32 sink_address = 2;     // unicast via this sink
optional BaseRole role = 10;          // SUBNODE / HEADNODE / SINK
optional bool is_cb_mac = 11;         // true = low latency, false = low energy
optional bool is_autorole = 12;
optional uint32 network_address = 13; // migration: new network id
optional uint32 network_channel = 14; // migration: new channel
optional bytes cipher_key = 15;       // migration: new network key
optional bytes authentication_key = 16;
optional uint32 node_address = 18;    // migration: re-address node
```

channel_map (17) is not supported on WM FW 3.x+.

### RemoteApiRequestCollection

```proto
optional string id = 1;                          // UUID
optional RemoteApiRequestType type = 2;          // BROADCAST | UNICAST
repeated RemoteApiRequest messages = 3;
optional uint32 resend_interval_s = 4;
optional bool is_cancel = 5;
optional bool is_close = 6;
optional MessageSendingType message_sending_type = 7; // INSTANT | DISTRIBUTED
optional uint32 start_time_s_epoch = 10;
optional uint32 activation_time_s_epoch = 11;
optional uint32 cancellation_time_s_epoch = 12;
```

Scheduled flow: configure (start time), activate later, cancel — mirrors
OTAP scheduling. `is_cancel`/`is_close` manage the request lifecycle.

### RemoteApiResponse

Results: `configure_result`, `activate_result`, `cancel_result`,
`metadata_update_result` — each a RemoteApiResponseId:

OK, ACCESS_DENIED, WRITE_ONLY_ATTRIBUTE, INVALID_BC_REQUEST, INVALID_BEGIN,
NO_SPACE_FOR_RESPONSE, INVALID_VALUE, INVALID_LENGTH, UNKNOWN_REQUEST,
UNKNOWN_ERROR.

### Migration workflow (network switch of existing nodes)

1. Create target network metadata (1042) if new.
2. Send Remote API request with new `network_address`, `network_channel`,
   `cipher_key`, `authentication_key` (+ optional `node_address`).
   Broadcast to a set or unicast per node.
3. Nodes acknowledge via remoteapi_response; they rejoin the new network.
4. Update node metadata (1061) / move id (1062) if the node identity
   mapping changes.
5. Watch RTS: nodes leave the old `(network_id, ...)` key and appear under
   the new one.

### Role change workflow

Same path with `role`, `is_cb_mac`, `is_autorole` fields — e.g. promote a
subnode to sink. Broadcast-capable.

## OTAP (firmware update)

Control messages (otap.proto) live on the RTS/metadata protobuf Message:

- `set_otap_state` (field 100) — SetOTAPState
- `set_otap_state_response` (field 101)
- `set_scratchpad_action` (field 102) — SetScratchpadAction
- `get_scratchpad_status` (field 91, metadata type 1074) — status polling
- `scratchpad_status` (field 65) — per-node results

Doc note: "Set OTAP state" / "Set scratchpad action" metadata message
types were added in WNT 4.0 and removed from the doc in 4.3; the protobuf
fields remain. Metadata types were likely 1073 (Set OTAP state) —
**verify against a live backend / WNT client traffic before implementing**.

### Scratchpad lifecycle

```
SCRATCHPAD_ACTION_NO_OTAP = 1
SCRATCHPAD_ACTION_PROPAGATE_ONLY = 2
SCRATCHPAD_ACTION_PROPAGATE_AND_PROCESS = 3
SCRATCHPAD_ACTION_PROPAGATE_AND_PROCESS_WITH_DELAY = 4
```

NPAD method (modern): delay options TEN_MINUTES, THIRTY_MINUTES, ONE_HOUR,
SIX_HOURS, ONE_DAY, TWO_DAYS, FIVE_DAYS.

### SetOTAPState

```proto
optional OTAPState state = 1;       // LOAD_SCRATCHPAD, ACTIVATE,
                                    // NPAD_LOAD_AND_ACTIVATE, CLOSE, ...
optional uint32 network_id = 2;
optional bytes scratchpad_bytes = 3;
optional uint32 resend_interval_s = 4;
optional bool is_sink_only = 5;
optional uint32 sequence_number = 6;
optional OTAPMethod method = 7;     // LEGACY | NPAD
optional uint32 scratchpad_crc = 8;
optional OTAPNPADDelay npad_delay = 9;
optional uint32 start_time_s_epoch = 10;
optional uint32 loading_time_s_epoch = 11;
optional uint32 activation_time_s_epoch = 12;
```

SetOTAPStateResponse mirrors gateway error codes (INVALID_SCRATCHPAD,
SINK_TIMEOUT, ...) per load/activate step.

### Monitoring OTAP progress (available today)

1. Metadata 1074: start continuous scratchpad query (resend_interval_s > 0)
   or single-shot (0) for a network; optional is_sink_only.
2. RTS `scratchpad_status` per node gives:
   - stored: size/crc/sequence/type/status
   - processed firmware + application versions (area_id, version x4, crc)
   - pending: scratchpad_action, target_sequence, target_crc,
     target_delay_minutes, remaining_time_minutes

TUI iteration 1: status view (query + table). Actual scratchpad upload/
activation is iteration 2+ (needs the missing metadata type verified).

## Downlink to nodes

- `Message.node_messages` (field 90): NodeMessage { destination_address,
  network_id, source_end_point, destination_end_point, qos, payload } —
  arbitrary application payload to a node. "Send data message" was
  deprecated from the doc in 4.4 but the protobuf field remains.
- NPD (1071): config data to sinks (app config + diagnostics interval).

## Backend-side error codes shared with gateway

AppConfigResponse / SetOTAPStateResponse reuse gateway error codes:
INVALID_SINK_ID, INVALID_NETWORK_ADDRESS/CHANNEL/KEYS, INVALID_SCRATCHPAD,
SINK_TIMEOUT, ACCESS_DENIED, ... (full list in message.proto /
otap.proto).
