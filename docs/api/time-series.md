# Time series service (InfluxDB, HTTP 8886)

History data for graphs. Direct Influx access — not a WNT-proprietary API.

## Access

- WNT < 4.4: Influx v1, same username/password as auth service.
- WNT >= 4.4: Influx v2, token from login response (`influx_token`).
  Token resets on WNT restart; a static token can be set via Influx v2
  management API.

Query with InfluxQL via HTTP GET, database `wirepas`:

```
GET https://<host>:8886/query?db=wirepas&epoch=ns&pretty=true&q=<InfluxQL>
Authorization: Token <token>          # WNT >= 4.4
```

Do not poll constantly — it can overload the backend. RTS/MQTT for live,
Influx on demand (opening the graph view / range change).

## Measurements

| Measurement | Content |
|---|---|
| analytics_boot | boot/startup diagnostics |
| analytics_next_hop | hop count to sink (WNT-calculated) |
| analytics_nodestate | node online info (OnlineStatus values) |
| analytics_packet | every received packet: counts, endpoints, addresses, payload size, times |
| analytics_traveltime_kpi | min/avg/max travel time per QoS |
| location_measurement | positioning measurement payloads from nodes |
| location_update | WPE-computed positions (needs WPE running) |
| node_metadata | node metadata changes (position, name, description, ...) |
| remote_api_response | Remote API response payloads |
| endpoint_<id> | application payloads per destination endpoint |

## Column naming

Columns are `Message_<field#>` or `Message_<nestedField#>_<field#>`,
decoded against message.proto field numbers:

- `Message_2` → `Message.network_id`
- `Message_7` → `Message.source_address`
- `Message_50_7` → `Message.diagnostics` (50) `.voltage` (7)
- `Message_50_3` → `Message.diagnostics.role`
- `Message_50_39` → `Message.diagnostics.sw_version_major`
- `Message_50_90` → `Message.diagnostics.boot_reason`
- `Message_71_1` → `Message.online_status.status`
- `Message_72_2` → `Message.traveltime_kpi` (field 72) `.qos0_average` (field 2)

Note: not every measurement stores the same subset of Message fields.
`analytics_traveltime_kpi` is a processed KPI table and may use different
column names than the raw protobuf numbering. Probe with
`SHOW FIELD KEYS ON wirepas` (and `SHOW MEASUREMENTS ON wirepas`) against
the live backend once, then pin the real names as constants in code.

Split in v2.0+: `analytics_packet.travel_time_ms` →
`travel_time_ms_qos0` / `travel_time_ms_qos1`.

## Graph queries for the TUI

Voltage over time (per node) — voltage is DiagnosticsData field 7
(`Message.diagnostics.voltage` → column `Message_50_7`), in whichever
measurement stores node diagnostics (confirm the measurement name on the
live backend):

```
SELECT mean("Message_50_7") FROM "<diagnostics measurement>"
  WHERE "Message_2" = <network> AND "Message_7" = <node>
  GROUP BY time(<range>) fill(null)
```

Travel time KPI (per network or node, QoS0 avg):

```
SELECT mean(...), min(...), max(...) FROM "analytics_traveltime_kpi" ...
```

Online state timeline: `analytics_nodestate` grouped by time.
Boot events: `analytics_boot` count per window; boot reason from
`Message_50_90`.
Hop count trend: `analytics_next_hop`.
Packet rate: `count()` on `analytics_packet` per window.

Exact field keys differ per measurement — first implementation step is to
probe `SHOW FIELD KEYS` against the live backend and pin them in code as
constants.

## Response shape

Standard Influx v1 query API JSON (`results[].series[]` with columns/tags/
values). Epoch ns (set `epoch=ns`).
