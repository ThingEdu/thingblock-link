# Agent Guide: thingblock-link

## What this is

`thingblock-link` is the local native **helper** (the "arduino-helper") that backs the
thingblock-editor firmware module's local-helper compile/flash mode. It is a single long-running
Rust binary with two faces: a **WebSocket server** for the browser/editor, and a **gRPC client**
to the `arduino-cli` daemon. It is a *translating proxy* — it turns a minimal JSON envelope from
the editor into arduino-cli gRPC calls and pumps streamed results back; it does not reimplement
any arduino logic.

This is a deliberately **separate repo** from the thingblock-editor monorepo. The two share a small,
fixed set of contracts — the WebSocket `{id, type, payload}` envelope (the proxy face) and the
static `/resources/<pack>/…` URL layout the editor loads from. Everything about the daemon stays
hidden from the JS side.

## Build, test, lint

Edition 2024. Standard Rust toolchain:

```sh
cargo build                              # debug build
cargo build --release                    # production build
cargo test                               # run tests
cargo clippy --all-targets -- -D warnings
cargo fmt                                # format (use --check in CI)
```

## Agent defaults

Use these unless the user asks otherwise:

1. Keep changes minimal and scoped to the request. Don't refactor, add features, or restyle code
   you weren't asked to touch.
2. This is a standalone binary, not a published library — restructure internals freely. The one
   contract to preserve is the WS protocol; don't change it casually.
3. Comments explain the current code, not its history. If something is counterintuitive, explain
   why it is correct now.
4. Fix root causes, not symptoms. Don't add fallbacks or validation for states that cannot happen.
5. When fixing a bug, add a failing test first (`#[test]` / `#[tokio::test]`), then fix until it
   and the rest of the suite pass.
6. Surface invalid states explicitly — prefer an explicit `Err` or `panic!` with a useful message
   over silent failure. Log actionable context (function, relevant IDs, key flags) via `tracing`:
   `warn!` for recoverable states, `error!` for invalid required data.
7. Validate only at boundaries — the WS envelope coming from the browser and responses from the
   arduino-cli daemon. Trust internal code.

## Conventions

- **Commits** follow [Conventional Commits](https://www.conventionalcommits.org/). (This is a
  convention here, not a hook-enforced rule as in the thingblock-editor monorepo.)
- Keep `Cargo.toml` sections (`dependencies`, etc.) in alphabetical order.
- Design docs live in `.agents/docs/`.

## Before submitting changes

- **Scope**: changes confined to the request; nothing extra added.
- **Build/test/lint clean**: `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check`.
- **Docs in sync**: if you change a convention or the architecture, update this file
  accordingly.
- **Commit format**: Conventional Commits.
