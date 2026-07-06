//! Coverage for the hardware-free part of `monitor`: the WS-payload → gRPC
//! `MonitorPortOpenRequest` mapping (serial port construction and the `baudrate`
//! port setting). The bidirectional gRPC translation needs a live daemon and a real
//! board, so it is exercised by manual end-to-end runs rather than here (the same
//! boundary `compile` and `upload` draw).

use thingblock_link::service::arduino::grpc::cli;
use thingblock_link::service::arduino::grpc::monitor::build_open_request;

fn instance() -> cli::Instance {
    cli::Instance { id: 7 }
}

#[test]
fn maps_port_as_serial() {
    let req = build_open_request(instance(), "/dev/ttyACM0", 9600);

    assert_eq!(req.instance, Some(instance()));
    let port = req.port.expect("port is set");
    assert_eq!(port.address, "/dev/ttyACM0");
    // Local-helper USB boards are always serial.
    assert_eq!(port.protocol, "serial");
}

#[test]
fn baud_rate_becomes_a_baudrate_setting() {
    let req = build_open_request(instance(), "/dev/ttyACM0", 115200);

    let config = req.port_configuration.expect("port configuration is set");
    assert_eq!(config.settings.len(), 1);
    assert_eq!(config.settings[0].setting_id, "baudrate");
    assert_eq!(config.settings[0].value, "115200");
}

#[test]
fn fqbn_left_empty_for_serial() {
    // Serial has a single built-in monitor, so no FQBN disambiguation is needed.
    let req = build_open_request(instance(), "/dev/ttyUSB0", 9600);
    assert!(req.fqbn.is_empty());
}
