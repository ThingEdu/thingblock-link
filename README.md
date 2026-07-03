# thingblock-link

The local native **helper** that backs the thingblock-editor firmware module's
local-helper compile/flash mode (the "arduino-helper"). It is a single
long-running Rust binary with two faces:

- a **WebSocket server** facing the browser/editor, and
- a **gRPC client** to the bundled [`arduino-cli`](https://github.com/arduino/arduino-cli)
  daemon.

It is a **translating proxy**: the browser speaks a minimal JSON envelope
(documented below) and the helper turns each request into an `ArduinoCoreService`
gRPC streaming call, pumping results back. The arduino-cli gRPC schema never leaks
to the browser, which keeps the daemon swappable and the JS side ignorant of
arduino-cli specifics.

The helper also spawns and owns the `arduino-cli daemon` child process — one
self-contained process for the user to run.

> **Design source of truth:** `.agents/docs/21-06_01.arduino-helper-design.md` owns
> the WS protocol, crate layout, and milestone roadmap. This README mirrors the
> protocol for quick reference; that document is authoritative.

## Status

The WS pipe and the daemon handshake are in place, along with `listBoards`,
`connect`, `disconnect`, `compile`, `upload`, `installPlatform`, `cancel`, and the
serial `monitor*` family. Every request type is now implemented. Each row in the
[reference](#websocket-protocol-reference) is tagged with its status. One-shot
reads live on a small [HTTP API](#http-api) sharing the same listener.

## Build, test, lint

Edition 2024. Standard Rust toolchain:

```sh
cargo build                              # debug build
cargo build --release                    # production build
cargo test                               # run tests (some need the bundled arduino-cli)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The bundled `arduino-cli` binaries live under `arduino-cli-binaries/`; integration
tests (`tests/daemon.rs`, `tests/ws.rs`) spawn the real daemon and are offline-safe.

**Linux build prerequisite.** The status window renders through `wry`, which builds
against the system WebView (WebKitGTK). Install its dev headers before building:

```sh
# Fedora
sudo dnf install webkit2gtk4.1-devel
# Debian/Ubuntu
sudo apt install libwebkit2gtk-4.1-dev
```

macOS (WKWebView) and Windows (WebView2) need no extra system package.

## Run

```sh
cargo run -- --port 3030                 # WS server on ws://localhost:3030/
```

`--port` defaults to `3030`. The process starts the arduino-cli daemon on an
OS-assigned loopback port, serves the WebSocket, and renders a tray icon
(`Starting…` → `Running on :PORT`, plus **Show Status** and Quit). **Show Status**
opens a small WebView window with the live helper status, the arduino-cli daemon's
health, and the connected-board count; closing it just hides it (the helper keeps
running). Quit reaps the daemon.

Set `RUST_LOG` (e.g. `RUST_LOG=thingblock_link=debug`) to adjust log verbosity.

## WebSocket protocol reference

The editor connects to `ws://localhost:<port>/` and exchanges JSON text frames.

### Envelope

Every message — in both directions — is:

```json
{ "id": "<string>", "type": "<string>", "payload": { } }
```

`id` correlates a request with its streamed responses and its single terminal
reply (`result` or `error`). Field names are camelCase. Unsolicited helper messages
(`event`) carry their own `id`.

### Client → helper

| `type` | `payload` | terminal reply | status |
| - | - | - | - |
| `listBoards` | `{ pnpid: string[] }` | `result { targets: ConnectionTarget[] }` | ✅ implemented |
| `connect` | `{ port: string }` | `result {}` | ✅ implemented |
| `disconnect` | `{}` | `result {}` | ✅ implemented |
| `compile` | `{ fqbn, options, source, libs }` | `result { artifact }` (after `log`/`progress`) | ✅ implemented |
| `upload` | `{ fqbn, port, uploadSpeed, artifact }` | `result {}` (after `log`) | ✅ implemented |
| `monitorOpen` | `{ port, baudRate }` | `result {}` then async `monitorData` | ✅ implemented |
| `monitorWrite` | `{ data }` | — | ✅ implemented |
| `monitorClose` | `{}` | `result {}` | ✅ implemented |
| `installPlatform` | `{ platform, version? }` | `result {}` (after `log`/`progress`) | ✅ implemented |
| `cancel` | `{}` (targets the request `id`) | `error { code: "cancelled" }` on the cancelled request | ✅ implemented |

### Helper → client

| `type` | `payload` | meaning |
| - | - | - |
| `log` | `{ chunk: string }` | streamed stdout/stderr for a request `id` |
| `progress` | `{ phase: string, percent: number }` | streamed progress for a request `id` |
| `result` | request-specific object | terminal success for a request `id` |
| `error` | `{ code: string, message: string }` | terminal failure for a request `id` |
| `monitorData` | `{ data: string }` | inbound serial bytes for the monitor session |
| `event` | request-specific object | unsolicited, e.g. `boardListChanged` |

`error.code` is one of `invalidRequest`, `daemon`, `grpc`, `cancelled`, `resource`,
or `io`. A malformed envelope is answered with `error{invalidRequest}` and an empty
`id` (there is nothing to correlate against).

### Shared shapes

```ts
// A connectable board returned by listBoards. Opaque to the JS Connection
// contract beyond the fields it reads.
type ConnectionTarget = { port: string; label: string };

// A compiled binary the editor hands back to `upload`.
type Artifact = { format: string; path: string };
```

### `listBoards`

`pnpid` is the list of accepted USB device ids, in Windows PNP form
(`USB\VID_xxxx&PID_xxxx`), from the device's upload config. The helper runs gRPC
`BoardList`, reconstructs each connected port's PNP id from its `vid`/`pid`
properties, and keeps the ports whose id matches an entry in `pnpid`
(case-insensitive). A port's `label` is its detected board name, falling back to
the port label, then its address.

**Request**

```json
{ "id": "1", "type": "listBoards", "payload": { "pnpid": ["USB\\VID_2341&PID_0043"] } }
```

**Reply** (an Uno on `/dev/ttyACM0`; empty `targets` when nothing matches)

```json
{ "id": "1", "type": "result", "payload": { "targets": [ { "port": "/dev/ttyACM0", "label": "Arduino Uno" } ] } }
```

### `connect` / `disconnect`

The arduino-cli daemon is connectionless per-port (the port is passed into each
compile/upload/monitor call; there is no "open a board" RPC), so `connect` is a
**helper-side session concept**. `connect {port}` is a lightweight store — it
checks the port is non-empty (`error{invalidRequest}` otherwise), records it as the
session's selected port, and replies `result {}`; it does **not** round-trip to the
daemon, so actual port existence surfaces later at upload/monitor time.
`disconnect {}` clears the selected port (and, once `monitor` lands, closes any open
monitor stream) and replies `result {}`.

### `installPlatform`

Downloads and installs a boards platform (core) via arduino-cli's board manager —
the on-demand path for cores that don't ship in the bundle (esp32). `platform` is
the `vendor:architecture` id (e.g. `esp32:esp32`); `version` is optional (latest
indexed release when absent). Streams `progress {phase, percent}` (per-file
download progress and install stages) and occasional `log` lines, then `result {}`.
Cancellable via `cancel`.

Installs are serialized daemon-wide: a second `installPlatform` while one runs — on
any session — is rejected with `error{invalidRequest}`. On success the helper
re-runs the daemon's `Init` before replying, so the freshly installed core is
immediately compilable. Install status is queried over the [HTTP API](#http-api),
not the WS.

## HTTP API

One-shot reads share the WS listener as plain HTTP GET routes (same CORS +
Private-Network-Access headers as `/resources`). Errors reuse the WS terminal's
`{code, message}` body (400 for a caller mistake, 502 for a daemon failure).

| route | reply |
| - | - |
| `GET /api/platforms` | `{ platforms: PlatformStatus[] }` — every indexed platform |
| `GET /api/platforms/{id}` | `PlatformStatus` for the exact `vendor:architecture` id, or 404 |

```ts
type PlatformStatus = {
  id: string;               // "esp32:esp32"
  name: string;             // "esp32"
  installed: boolean;
  installedVersion?: string;
  latestVersion?: string;
};
```

## Architecture

```
src/
  main.rs        thin binary: tracing, clap --port, hands main thread to the tray
  daemon.rs      spawns/owns arduino-cli daemon, gRPC channel, Create/Init handshake
  grpc.rs, grpc/ generated `pb` module + `Client` wrapper; one submodule per RPC
    board.rs       BoardList -> pnpid filter -> ConnectionTarget[]
    platform.rs    PlatformSearch/PlatformInstall -> status + install events
  ws.rs, ws/
    server.rs      axum accept loop -> Session per socket
    session.rs     per-connection state + read/dispatch/write pipe
    protocol.rs    serde structs for the JSON envelope (the cross-repo contract)
    api.rs         one-shot HTTP JSON routes (/api/platforms)
  bridge.rs      envelope <-> gRPC translation; the only place the two schemas meet
  tray.rs        tray-icon status/quit UI + main-thread tao event loop
  window.rs      wry WebView status window (opened from the tray)
  error.rs
assets/          status.html — the status window's markup/CSS/JS
tests/           integration tests (no inline #[cfg(test)] in src/)
```

Stack: `tokio`, `tonic` / `prost`, `axum`, `serde`, `tracing`, `clap`,
`tao` / `tray-icon` / `wry`.

## License

Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0). See
[`LICENSE`](LICENSE) for the full text.
