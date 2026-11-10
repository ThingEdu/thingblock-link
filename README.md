# thingblock-link

A local helper that lets the thingblock editor compile and flash boards. The browser talks to it
over WebSocket and HTTP, and it passes the work to a bundled
[`arduino-cli`](https://github.com/arduino/arduino-cli).

## Table of contents

- [Prerequisites](#prerequisites)
- [Build, test, lint](#build-test-lint)
- [Run](#run)
- [License](#license)

## Prerequisites

- **Rust 1.85+** (the crate uses edition 2024).
- **WebKitGTK dev headers on Linux**, which the status window needs:

  ```sh
  sudo dnf install webkit2gtk4.1-devel        # Fedora
  sudo apt install libwebkit2gtk-4.1-dev      # Debian/Ubuntu
  ```

  macOS and Windows need nothing extra.

You don't need to install `arduino-cli` or `protoc` yourself. The `arduino-cli` binaries are
bundled in `arduino-cli-binaries/`, and the gRPC code is generated in pure Rust.

## Build, test, lint

```sh
cargo build [--release]
cargo test                                # some tests spawn the bundled arduino-cli
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Run

```sh
cargo run -- --port 3030                  # default port is 3030
```

This starts the arduino-cli daemon, serves `ws://localhost:3030/` and shows a tray icon. **Show
Status** opens a window with the helper, daemon and connected-board status. **Quit** stops the
daemon. Use `RUST_LOG` (e.g. `RUST_LOG=thingblock_link=debug`) to set log verbosity.

## License

AGPL-3.0-only (GNU Affero General Public License v3.0). See [`LICENSE`](LICENSE).
