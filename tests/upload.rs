//! Coverage for the hardware-free part of `upload`: the WS-payload → gRPC
//! `UploadRequest` mapping (port construction, `import_file`, and the optional
//! `upload.speed` override). The streamed gRPC translation needs a live daemon and
//! a real board, so it is exercised by manual end-to-end runs rather than here
//! (the same boundary `compile` draws).

use thingblock_link::service::arduino::grpc::cli;
use thingblock_link::service::arduino::grpc::upload::build_request;

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
