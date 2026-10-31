# MQTT JSON API (8883 TCP / 9002 WS)

Alternative to the protobuf RTS stream: full node objects as JSON on a
single topic. Possible future backend for the TUI (simpler decoding, no
protobuf codegen), at the cost of a different auth story (dedicated
`mqttjsonuser` profile) and coarser updates.

## Basics

- Topic: `node_data_json`, QoS 1 (at-least-once → dedupe on client),
  content type `application/json`.
- Each message: `{"version": 1, "nodes": [ ...full node objects... ]}`.
- Initial state for all nodes at startup, then changed nodes every 5 s.
- Live fields only (no history — use Influx for that).

## Modes

- Normal (default): updates on role, location, voltage, metadata,
  online status changes.
- Optimized (`wnt_mqtt_json_is_optimized_on`): only tag location changes
  (lat/lon/alt/positioning time) + anchor battery voltage.
- `wnt_mqtt_json_forward_calcualted_anchor_locations` (sic): include
  WPE-calculated anchor positions.

## Node object fields

network_name, node_name, is_approved, is_virtual, online_status,
online_status_string, voltage, latitude, longitude, altitude,
position_pixel_x/y, position_meter_x/y, positioning_role,
positioning_role_string, measurement_time(_epoch), positioning_time(_epoch),
building_id/name, floor_plan_id/name, areas[{id,name}], node_address,
network_address; >=4.1: sink_address, gateway_id (sinks);
>=4.4 opportunistic anchors: anchor_calculated_latitude/longitude/altitude,
anchor_location_error_m.

## TUI relevance

Iteration 1 uses the RTS protobuf stream (authoritative, richer
diagnostics: neighbors, events, scratchpad, FW versions). MQTT JSON could
power a simplified read-only mode later; skip for now.
