# HTTP API

One-shot reads and static pack files served by the local gateway. Streaming and stateful work
(compile, upload, monitor, platform install) lives on the [Arduino WebSocket](ws-arduino.md)
instead.

- **Base URL:** `http://127.0.0.1:3030` (port overridable with `--port`; bound on localhost only).
- **CORS:** the request `Origin` is mirrored, any method/header is allowed, and Private Network
  Access is enabled so a public editor origin can reach localhost.
- **Bodies:** JSON, camelCase fields.

## Errors

Every failing route returns a non-2xx status with:

```json
{ "code": "invalidRequest", "message": "pnpid is required" }
```

| Status | `code`           | Meaning                                   |
| ------ | ---------------- | ----------------------------------------- |
| 400    | `invalidRequest` | The request is missing or has a bad field |
| 404    | `notFound`       | The named resource doesn't exist          |
| 502    | `daemonError`    | The arduino-cli daemon call failed        |

## `GET /api/platforms`

Every platform this build supports, with its install status.

```json
{
  "platforms": [
    {
      "id": "arduino:avr",
      "name": "Arduino AVR Boards",
      "installed": true,
      "installedVersion": "1.8.6",
      "latestVersion": "1.8.6"
    }
  ]
}
```

| Field              | Type           | Notes                                                  |
| ------------------ | -------------- | ------------------------------------------------------ |
| `id`               | string         | `vendor:architecture`; the value WS `installPlatform` takes |
| `name`             | string         | Human-readable; falls back to `id` if not indexed      |
| `installed`        | bool           |                                                        |
| `installedVersion` | string \| null |                                                        |
| `latestVersion`    | string \| null | `null` if the platform isn't in the package indexes    |

Supported ids: `arduino:avr`, and `esp32:esp32` in builds with ESP32 enabled.

Errors: `502 daemonError`.

## `GET /api/platforms/{id}`

One supported platform by id, e.g. `/api/platforms/esp32:esp32`. The body is a single platform
object as above (not wrapped).

Errors: `404 notFound` (unknown or unsupported id), `502 daemonError`.

## `GET /api/boards?pnpid=…`

Connected boards whose USB id matches one of the given Windows-style PNP ids.

| Query   | Required | Notes                                                                 |
| ------- | -------- | --------------------------------------------------------------------- |
| `pnpid` | yes      | Comma-separated, e.g. `USB\VID_2341&PID_0043,USB\VID_1A86&PID_7523`; matched case-insensitively |

```json
{
  "targets": [
    { "port": "/dev/ttyUSB0", "label": "Arduino Uno" }
  ]
}
```

`port` is the address WS `connect` takes. Ports without a USB vid/pid (e.g. network ports) are
never listed.

Errors: `400 invalidRequest` (`pnpid` missing or empty), `502 daemonError`.

## `GET /resources/<pack>/…`

Static files from the resource root (the `thingblock-resource` dir beside the executable, or
`--resource-root`). Responses carry `Cache-Control: no-cache`: packs are updated in place under
stable URLs, so clients revalidate and get `304` when unchanged.
