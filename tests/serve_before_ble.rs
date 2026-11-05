//! The HTTP and WS faces must answer while BLE discovery is still pending. On macOS the first
//! run of a new binary blocks discovery on the Bluetooth permission prompt, which nobody may be
//! there to answer; the editor's resources, examples and lessons must not wait on it.

use std::fs;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use thingblock_link::handler::arduino::ArduinoHandler;
use thingblock_link::handler::thingblock::ThingblockHandler;
use thingblock_link::handler::thingblock::resource::ResourceRoot;
use thingblock_link::server;
use thingblock_link::utils::tempdir::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn serves_resources_while_ble_discovery_hangs() {
    let resources = TempDir::new("thingblock-link-ble-pending").expect("temp resource dir");
    fs::write(resources.path().join("hello.txt"), "hi").expect("write file");
    let thingblock = Arc::new(ThingblockHandler::new(
        ResourceRoot::new(resources.path()).expect("resource root"),
    ));
    let arduino = Arc::new(
        ArduinoHandler::start(None)
            .await
            .expect("daemon should start"),
    );

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        // A discovery that never finishes, as when the permission prompt is left unanswered.
        server::router::serve_with_ble(listener, arduino, thingblock, std::future::pending())
            .await
            .expect("serve");
    });

    let request = async {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream
            .write_all(b"GET /resources/hello.txt HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write request");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.expect("read response");
        String::from_utf8_lossy(&raw).into_owned()
    };
    let response = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .expect("the server must answer without waiting for BLE");
    assert!(response.starts_with("HTTP/1.0 200"), "got: {response}");
    assert!(response.ends_with("hi"), "got: {response}");
}
