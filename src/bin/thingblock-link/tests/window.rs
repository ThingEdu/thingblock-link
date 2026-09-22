//! Pins the status window's JSON payloads, which go straight into `evaluate_script` and are
//! a contract with `assets/status.html` (field names and the `state` strings it switches on).

use crate::ui::window::{StatusView, Telemetry};

#[test]
fn running_status_serializes_state_and_port() {
    let json = serde_json::to_value(StatusView {
        state: "running",
        port: Some(3030),
        message: None,
    })
    .unwrap();

    assert_eq!(json["state"], "running");
    assert_eq!(json["port"], 3030);
    assert!(json["message"].is_null());
}

#[test]
fn failed_status_carries_message() {
    let json = serde_json::to_value(StatusView {
        state: "failed",
        port: None,
        message: Some("daemon not reachable".into()),
    })
    .unwrap();

    assert_eq!(json["state"], "failed");
    assert_eq!(json["message"], "daemon not reachable");
}

#[test]
fn telemetry_serializes_boards_and_health() {
    let json = serde_json::to_value(Telemetry {
        boards: 2,
        healthy: true,
    })
    .unwrap();

    assert_eq!(json["boards"], 2);
    assert_eq!(json["healthy"], true);
}
