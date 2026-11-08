# WebSocket protocol

The editor's long-lived channel to the helper for stateful and streaming work: binding a board,
compiling, flashing, the serial monitor and platform installs. One-shot reads (platforms, board
list) are on the [HTTP API](api.md).

- **URL:** `ws://127.0.0.1:3030/ws` (port overridable with `--port`).
- **Frames:** text frames, one JSON message each. Binary frames are ignored.
- **Session:** each socket is one session with its own connected port, serial monitor and
  in-flight requests. Closing the socket does a [`disconnect`](#disconnect).

## Envelope

Every message, in both directions:

```json
{ "id": "7", "type": "compile", "payload": { … } }
```

| Field     | Type   | Notes                                                                |
| --------- | ------ | -------------------------------------------------------------------- |
| `id`      | string | Chosen by the editor per request; every response to it carries the same `id` |
| `type`    | string | Message type, camelCase                                              |
| `payload` | object | Type-specific, camelCase fields; `{}` when there is nothing to carry |

Use a fresh `id` for each request: while a request is running its `id` is how responses are told
apart and how [`cancel`](#cancel) finds it.

## Request lifecycle

A request receives zero or more **streamed** responses, then exactly one **terminal** response:

| `type`        | Kind     | Payload                                   |
| ------------- | -------- | ----------------------------------------- |
| `log`         | streamed | `{ "chunk": string }` — tool output text  |
| `progress`    | streamed | `{ "phase": string, "percent": number }`  |
| `monitorData` | streamed | `{ "data": string }` — serial input       |
| `result`      | terminal | Per request, see below; `{}` if none      |
| `error`       | terminal | `{ "code": string, "message": string }`   |

Exceptions: `monitorWrite` and `cancel` get no response of their own, and `monitorData` streams
under the `monitorOpen` id after that request's `result`.

Consecutive `log` / `monitorData` chunks of one `id` may be merged into a single frame (up to
100 ms of output). Chunk boundaries carry no meaning; order is always preserved.

### Error codes

| `code`           | Meaning                                                              |
| ---------------- | -------------------------------------------------------------------- |
| `invalidRequest` | Malformed envelope, or the request isn't valid in the session's state |
| `cancelled`      | The request was stopped by `cancel` or `disconnect`                  |
| `failed`         | The daemon, the board, or a resource file failed                     |

A malformed envelope (bad JSON, unknown `type`, wrong payload shape) gets `error` with
`code: "invalidRequest"` and an empty `id`, since its `id` couldn't be read.

## Session state

```
          connect                     disconnect / socket close
  idle ───────────────▶ connected ─────────────────────────────▶ idle
                         port = P      closes monitor, cancels all requests
```

`compile` and `installPlatform` work in any state. `upload`, `flashFirmware` and `monitorOpen`
require a connected port and use it; they fail with `invalidRequest` otherwise.

## Requests

### `connect`

Binds the session to a board port after checking that a board with one of the given PNP ids is
on that port.

```json
{ "port": "/dev/ttyUSB0", "pnpid": ["USB\\VID_1A86&PID_7523"] }
```

- `result`: `{}`
- `invalidRequest`: already connected (send `disconnect` first), or no matching board on `port`.

`port` is a `targets[].port` from `GET /api/boards`.

### `disconnect`

Closes the serial monitor, cancels every in-flight request of this session (their terminal is
`error{cancelled}`), and clears the connected port. Payload `{}`.

- `result`: `{}`, also when not connected.

### `compile`

Compiles a sketch. Doesn't need a connected port. Cancellable.

```json
{
  "fqbn": "esp32:esp32:esp32",
  "source": "void setup() {}\nvoid loop() {}\n",
  "options": { "verbose": false },
  "libs": [{ "pack": "thingbot", "lib": "libraries/ThingBot" }]
}
```

| Field     | Required | Notes                                                       |
| --------- | -------- | ----------------------------------------------------------- |
| `fqbn`    | yes      | Including board options, e.g. `esp32:esp32:esp32:PartitionScheme=huge_app` |
| `source`  | yes      | The `.ino` contents                                         |
| `options` | yes      | Every field optional, unknown keys ignored; see below       |
| `libs`    | no       | Vendored library dirs under the resource root, `lib` relative to `pack` |

`options`:

| Field             | Type     | Notes                                      |
| ----------------- | -------- | ------------------------------------------ |
| `verbose`         | bool     | Verbose arduino-cli output                 |
| `warnings`        | string   | `none`, `default`, `more` or `all`         |
| `libraries`       | string[] | Extra library root directories             |
| `buildProperties` | string[] | `key=value` build properties               |

Streams `log` and `progress`. `result`:

```json
{
  "artifact": {
    "format": "bin",
    "path": "/tmp/arduino/sketches/…/sketch_7.ino.bin",
    "parts": [{ "offset": 65536, "path": "…" }]
  }
}
```

`format` is `hex` or `bin`. `parts` lists the separately addressed ESP images (bootloader,
partitions, boot_app0, app) and is omitted for single-image targets. Treat the artifact as opaque
and pass it back to `upload` unchanged.

- `failed`: a referenced lib is missing or outside the resource root, the compile failed, or no
  artifact was produced.

### `upload`

Flashes a compiled artifact to the connected port. Cancellable.

```json
{ "fqbn": "esp32:esp32:esp32", "uploadSpeed": 921600, "artifact": { … } }
```

`fqbn` must be the one the artifact was compiled with. `uploadSpeed` `0` uses the board's default.
Streams `log`. `result`: `{}`.

- `invalidRequest`: not connected.

### `flashFirmware`

Flashes a prebuilt image shipped in a pack to the connected port, without compiling. Cancellable.

```json
{ "fqbn": "esp32:esp32:esp32", "uploadSpeed": 921600, "pack": "thingbot", "file": "firmware/app.bin" }
```

`file` is relative to `pack` under the resource root. Streams `log`. `result`: `{}`.

- `invalidRequest`: not connected.
- `failed`: the image is missing or outside the resource root, or the flash failed.

### `monitorOpen`

Opens the serial monitor on the connected port.

```json
{ "baudRate": 115200 }
```

`result` `{}` is sent once the port is open. From then on serial input streams as `monitorData`
under this request's `id` until the monitor is closed. If the port fails later (e.g. the board is
unplugged), the stream ends with `error{failed}` under the same `id`.

One monitor per session: opening while one is open closes the old one first, and its
`monitorData` stream simply stops.

- `invalidRequest`: not connected.

### `monitorWrite`

Sends text to the open monitor. No response.

```json
{ "data": "hello\n" }
```

- `invalidRequest`: no monitor open.

### `monitorClose`

Closes the monitor and releases the port; the connection stays. Payload `{}`.

- `result`: `{}`, also when no monitor is open.

### `installPlatform`

Installs a supported boards platform at the version this build pins. Cancellable. Only one
install runs at a time across all sessions.

```json
{ "platform": "esp32:esp32" }
```

`platform` is an `id` from `GET /api/platforms`. Streams `log` and `progress`. `result`: `{}`,
sent once the platform is usable by `compile`.

- `invalidRequest`: unknown or unsupported `platform` (as a malformed envelope, empty `id`), or
  another install is running.

### `cancel`

Stops the in-flight request whose `id` equals this message's `id`. Payload `{}`. No response of its
own: the cancelled request ends with `error{cancelled}`. Does nothing if no such request is
running. Applies to `compile`, `upload`, `flashFirmware` and `installPlatform`.

```json
{ "id": "7", "type": "cancel", "payload": {} }
```
