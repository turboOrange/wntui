# Authentication service (WS 8813, JSON)

Handles login and user management. Returns a `session_id` used by the
metadata and RTS services. The session dies when the auth connection
closes; already-open connections stay alive.

## Login (type 1)

Request:

```json
{
  "data": { "username": "<user>", "password": "<password>" },
  "type": 1,
  "version": 5
}
```

Response (WNT < 4.4):

```json
{
  "data": { "role": 1, "session_id": "<session id>" },
  "result": 1,
  "type": 1,
  "version": 5
}
```

Response (WNT >= 4.4) — adds Influx v2 token for the time series service:

```json
{
  "data": {
    "role": 1,
    "session_id": "<session id>",
    "influx_token": "<token>"
  },
  "result": 1,
  "type": 1,
  "version": 5
}
```

- `role`: 1 = Administrator, 2 = Operator.
- `influx_token` is reset on every WNT service restart. For a stable token,
  configure a static one directly in Influx v2 via its management API.

### Credentials rules

| Field | Min | Max |
|---|---|---|
| username | 1 | 63 |
| password | 6 | 255 |
| full_name | 1 | 255 |

## User management (admin only)

All request `data` objects support `originator_token` (client UUID string)
to correlate with RTS notifications.

| type | Message | Notes |
|---|---|---|
| 11 | Get users | returns users[]: username, full_name, role |
| 12 | Create user | data.users[] with username, password, role, full_name |
| 13 | Update user | username required; other fields optional |
| 14 | Delete user | data.users[] with username |

User changes fan out to other clients via RTS
`metadata_update_message.added_or_changed_users` / `deleted_users`.

## Result codes

| Code | Meaning |
|---|---|
| 1 | Ok |
| 2 | Generic error |
| 3 | Invalid credentials |
| 4 | Wrong protocol version |
| 5 | User does not have rights |
| 6 | Invalid user id |
| 7 | User already exists |
| 8 | Received message was invalid |
| 9 | Invalid session id |

## Session semantics (important for the client)

- session_id works for metadata (8812) and RTS (8811) handshakes.
- Closing the auth socket invalidates the session for *new* connections;
  existing sockets keep working.
- Practical consequence: keep the auth socket open for the client lifetime,
  or re-login when establishing new metadata/RTS connections after a drop.

## TUI auth flow (planned)

1. Config file stores host + username (not password).
2. Password sources, in order: `WNT_PASSWORD` env var → interactive hidden
   prompt (CLI flag `--password` discouraged; visible in shell history).
3. Login once at startup; hold auth WS open; reconnect = re-login.
4. `influx_token` kept in memory only for graph queries.
