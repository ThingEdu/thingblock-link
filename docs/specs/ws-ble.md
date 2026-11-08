# BLE WebSocket protocol

The editor device extensions' channel to the helper's Bluetooth Low Energy radio: scanning,
connecting, reading/writing characteristics and receiving notifications.

- **URL:** `ws://127.0.0.1:3030/ws/ble` (port overridable with `--port`).
- **Frames:** text frames, one JSON message each. Binary frames are ignored.
- **Session:** each socket holds its own scans and connected devices. Closing the socket stops
  its scans and disconnects its devices.
- **No radio:** the socket still opens, but every request fails with `error{failed}`
  (`no BLE adapter available`).

## Envelope

Same as the [Arduino WebSocket](ws-arduino.md#envelope): `{ "id": string, "type": string,
"payload": object }`, camelCase, a fresh `id` per request.

## Responses

| `type`         | Kind        | `id`               | Payload                                                  |
| -------------- | ----------- | ------------------ | -------------------------------------------------------- |
| `device`       | streamed    | the `scan`'s id    | `{ deviceId, name: string \| null, rssi: number \| null }` |
| `notify`       | unsolicited | `""`               | `{ deviceId, characteristic: uuid, data: base64 }`       |
| `disconnected` | unsolicited | `""`               | `{ deviceId }` — the device dropped on its own           |
| `result`       | terminal    | the request's id   | Per request, see below; `{}` if none                     |
| `error`        | terminal    | the request's id   | `{ code, message }`                                      |

Each response is its own frame; nothing is merged.

### Error codes

| `code`           | Meaning                                                                    |
| ---------------- | -------------------------------------------------------------------------- |
| `invalidRequest` | Malformed envelope (incl. a bad UUID), bad base64, or a device not connected / already connected |
| `failed`         | No radio, or the adapter or device failed (incl. an unknown characteristic) |

A malformed envelope gets `error{invalidRequest}` with an empty `id`.

## Conventions

- **UUIDs** (`services`, `service`, `characteristic`) are full 128-bit strings, e.g.
  `6e400001-b5a3-f393-e0a9-e50e24dcca9e`. 16-bit short forms are not accepted.
- **Bytes** (`data`) are standard base64.
- **`deviceId`** is opaque: take it from a `device` hit and pass it back unchanged.

## Requests

### `scan`

Scans for devices advertising any of `services`, or every device if `services` is empty.

```json
{ "services": ["6e400001-b5a3-f393-e0a9-e50e24dcca9e"], "namePrefix": "ThingBot" }
```

`namePrefix` is optional and matches the start of the advertised name. Streams `device` hits
(a device can repeat as its RSSI updates) until [`cancel`](#cancel)led or the adapter stops the
scan, then `result {}`. The radio runs one scan filter at a time: starting a scan replaces the
filter of any running scan, including another socket's, which may then stop getting hits.

- `failed`: the scan couldn't start.

### `connect`

Connects a device found by a scan and starts relaying its notifications.

```json
{ "deviceId": "…" }
```

`result`: `{ "deviceId": "…" }`.

- `invalidRequest`: already connected on this socket.
- `failed`: the device wasn't seen by a scan, or the connection failed.

### `disconnect`

```json
{ "deviceId": "…" }
```

`result`: `{}`, also for a device that isn't connected.

### `write`

```json
{
  "deviceId": "…",
  "service": "6e400001-b5a3-f393-e0a9-e50e24dcca9e",
  "characteristic": "6e400002-b5a3-f393-e0a9-e50e24dcca9e",
  "data": "AQI=",
  "withResponse": false
}
```

`withResponse` is optional (default `false`). `result`: `{}`.

### `read`

```json
{ "deviceId": "…", "service": "…", "characteristic": "…" }
```

`result`: `{ "data": base64 }`.

### `subscribe` / `unsubscribe`

```json
{ "deviceId": "…", "service": "…", "characteristic": "…" }
```

Turns `notify` messages for that characteristic on or off. `result`: `{}`.

`write`, `read`, `subscribe` and `unsubscribe` fail with `invalidRequest` when the device isn't
connected on this socket.

### `cancel`

Stops the scan whose `id` equals this message's `id`. Payload `{}`. No response of its own: the
scan ends with `result {}`. Does nothing if no such scan is running.

```json
{ "id": "3", "type": "cancel", "payload": {} }
```
