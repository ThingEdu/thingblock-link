//! Coverage for the hardware-free part of `upload`: the WS-payload → gRPC
//! `UploadRequest` mapping (port construction, `import_file`, and the optional
//! `upload.speed` override). The streamed gRPC translation needs a live daemon and
//! a real board, so it is exercised by manual end-to-end runs rather than here
//! (the same boundary `compile` draws).
//!
//! Also covers `flashFirmware`'s boundary check: it resolves its `{pack, file}`
//! through `ResourceRoot::resolve_firmware_file` on the read loop before spawning
//! the shared upload pump, so a path leaving the resource root is refused
//! synchronously. That needs a live WS session, so this file also carries the
//! same session-standup/round-trip harness `connect.rs`/`ws.rs` use (no shared
//! `tests/common` module exists yet, so it's inlined here per that idiom).

use std::fs;
use std::net::Ipv4Addr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use serde_json::json;
use thingblock_link::server;
use thingblock_link::service::arduino::daemon::Daemon;
use thingblock_link::service::arduino::grpc::cli;
use thingblock_link::service::arduino::grpc::upload::build_request;
use thingblock_link::service::resource::ResourceRoot;
use thingblock_link::utils::tempdir::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Stand up a live daemon + WS server over a temp resource root and connect one
/// session. Returns the socket plus the resource-root temp-dir guard, whose path
/// is also the root fixtures are built under.
async fn connect_session() -> (Socket, TempDir) {
    let daemon = Arc::new(Daemon::start(None).await.expect("daemon should start"));
    let resources = TempDir::new("thingblock-link-flash-test").expect("temp resource dir");
    let resource_root = Arc::new(ResourceRoot::new(resources.path()).expect("resource root"));

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind ws listener");
    let addr = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        server::router::serve(listener, daemon, resource_root)
            .await
            .expect("serve");
    });

    let url = format!("ws://{addr}/");
    let (socket, _resp) = connect_async(url.as_str()).await.expect("ws connect");
    (socket, resources)
}

/// Send one `{id, type, payload}` request and return the first text reply parsed
/// as JSON.
async fn request(
    ws: &mut Socket,
    id: &str,
    kind: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let envelope = json!({ "id": id, "type": kind, "payload": payload }).to_string();
    ws.send(Message::Text(envelope.into()))
        .await
        .expect("send request");
    loop {
        match ws.next().await.expect("stream ended").expect("ws message") {
            Message::Text(text) => {
                return serde_json::from_str(text.as_str()).expect("parse reply");
            }
            _ => continue,
        }
    }
}

#[tokio::test]
async fn flash_firmware_refuses_an_image_outside_the_resource_root() {
    let (mut ws, guard) = connect_session().await;

    // A sibling dir outside the root that `../` would reach, holding a real file so
    // canonicalize succeeds on a real path and the containment check — not a
    // missing-path error — is what rejects it. Discovered by name rather than a
    // literal `../../../../etc/hosts`: that depends on the OS temp dir's nesting
    // depth and does not reliably reach an existing path on every machine (the same
    // fixture shape `resource.rs`'s `resolve_firmware_file_refuses_a_path_escaping_the_root`
    // uses, for the same reason).
    let outside = TempDir::new("thingblock-link-flash-test-outside").expect("temp dir");
    fs::write(outside.path().join("secret.bin"), b"\x00\x01").expect("write outside file");
    let outside_name = outside
        .path()
        .file_name()
        .expect("outside dir has a name")
        .to_str()
        .expect("name is utf8");
    fs::create_dir_all(guard.path().join("extensions/devices/thingbot")).expect("create pack dir");

    let reply = request(
        &mut ws,
        "1",
        "flashFirmware",
        json!({
            "fqbn": "esp32:esp32:esp32c3",
            "port": "/dev/null",
            "uploadSpeed": 921600,
            "pack": "extensions/devices/thingbot",
            "file": format!("../../../../{outside_name}/secret.bin")
        }),
    )
    .await;

    assert_eq!(
        reply["type"], "error",
        "a path leaving the root must be refused"
    );
    assert!(
        reply["payload"]["message"]
            .as_str()
            .unwrap()
            .contains("escapes the resource root"),
        "error should name the containment failure, got: {reply}"
    );
}

fn instance() -> cli::Instance {
    cli::Instance { id: 7 }
}

#[test]
fn maps_fqbn_artifact_and_port() {
    let req = build_request(
        instance(),
        "arduino:avr:uno",
        "/tmp/sketch/build/sketch.ino.hex",
        "/dev/ttyACM0",
        0,
    );

    assert_eq!(req.instance, Some(instance()));
    assert_eq!(req.fqbn, "arduino:avr:uno");
    // The artifact goes in `import_file` (overrides sketch_path/import_dir).
    assert_eq!(req.import_file, "/tmp/sketch/build/sketch.ino.hex");
    assert!(req.sketch_path.is_empty());

    let port = req.port.expect("port is set");
    assert_eq!(port.address, "/dev/ttyACM0");
    // Local-helper USB boards are always serial.
    assert_eq!(port.protocol, "serial");
}

#[test]
fn zero_upload_speed_defers_to_fqbn() {
    let req = build_request(
        instance(),
        "arduino:avr:uno",
        "/b/s.ino.hex",
        "/dev/ttyACM0",
        0,
    );
    assert!(
        req.upload_properties.is_empty(),
        "0 means let the FQBN's boards.txt decide; no override emitted"
    );
}

#[test]
fn nonzero_upload_speed_overrides_via_property() {
    let req = build_request(
        instance(),
        "esp32:esp32:esp32",
        "/b/s.ino.bin",
        "/dev/ttyUSB0",
        921600,
    );
    assert_eq!(req.upload_properties, ["upload.speed=921600"]);
}
