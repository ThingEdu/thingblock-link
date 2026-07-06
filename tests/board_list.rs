//! Unit coverage for the `BoardList` → `ConnectionTarget` translation. Pure and
//! hardware-free: it drives `detected_ports_to_targets` with constructed
//! arduino-cli port fixtures so the pnpid filter and labeling are exercised
//! without a daemon or a plugged-in board.

use std::collections::HashMap;

use thingblock_link::service::arduino::grpc::board::{
    count_board_ports, detected_ports_to_targets,
};
use thingblock_link::service::arduino::grpc::cli::{BoardListItem, DetectedPort, Port};

/// An Uno-shaped serial port: VID 2341 / PID 0043, with a matching board.
fn uno_port() -> DetectedPort {
    DetectedPort {
        matching_boards: vec![BoardListItem {
            name: "Arduino Uno".into(),
            fqbn: "arduino:avr:uno".into(),
            ..Default::default()
        }],
        port: Some(Port {
            address: "/dev/ttyACM0".into(),
            label: "ttyACM0".into(),
            properties: HashMap::from([
                ("vid".into(), "0x2341".into()),
                ("pid".into(), "0x0043".into()),
            ]),
            ..Default::default()
        }),
    }
}

const UNO_PNPID: &str = "USB\\VID_2341&PID_0043";

#[test]
fn keeps_matching_port_and_uses_board_name_label() {
    let targets = detected_ports_to_targets(&[uno_port()], &[UNO_PNPID.into()]);

    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].port, "/dev/ttyACM0");
    assert_eq!(targets[0].label, "Arduino Uno");
}

#[test]
fn drops_port_whose_pnpid_is_not_requested() {
    let targets = detected_ports_to_targets(&[uno_port()], &["USB\\VID_FFFF&PID_FFFF".into()]);

    assert!(targets.is_empty());
}

#[test]
fn matches_pnpid_case_insensitively() {
    let targets = detected_ports_to_targets(&[uno_port()], &["usb\\vid_2341&pid_0043".into()]);

    assert_eq!(targets.len(), 1, "match must ignore ASCII case");
}

#[test]
fn skips_port_missing_vid_or_pid() {
    let network_port = DetectedPort {
        matching_boards: vec![],
        port: Some(Port {
            address: "192.168.1.5".into(),
            label: "my-board".into(),
            // No vid/pid properties — cannot form a pnpid.
            ..Default::default()
        }),
    };

    let targets = detected_ports_to_targets(&[network_port], &[UNO_PNPID.into()]);

    assert!(targets.is_empty());
}

#[test]
fn labels_fall_back_to_port_label_then_address() {
    let mut no_board = uno_port();
    no_board.matching_boards.clear();
    let label_fallback = detected_ports_to_targets(&[no_board], &[UNO_PNPID.into()]);
    assert_eq!(label_fallback[0].label, "ttyACM0");

    let mut no_board_no_label = uno_port();
    no_board_no_label.matching_boards.clear();
    no_board_no_label.port.as_mut().unwrap().label = String::new();
    let address_fallback = detected_ports_to_targets(&[no_board_no_label], &[UNO_PNPID.into()]);
    assert_eq!(address_fallback[0].label, "/dev/ttyACM0");
}

#[test]
fn empty_pnpid_filter_matches_nothing() {
    let targets = detected_ports_to_targets(&[uno_port()], &[]);

    assert!(targets.is_empty());
}

/// A port with no USB vid/pid (e.g. a network port) — not a board.
fn network_port() -> DetectedPort {
    DetectedPort {
        matching_boards: vec![],
        port: Some(Port {
            address: "192.168.1.5".into(),
            label: "net".into(),
            ..Default::default()
        }),
    }
}

#[test]
fn board_count_includes_only_usb_ports() {
    // Two USB boards + one network port → 2 (the count is unfiltered by pnpid,
    // unlike the listBoards path, but still excludes non-USB serial devices).
    let ports = [uno_port(), uno_port(), network_port()];
    assert_eq!(count_board_ports(&ports), 2);
}

#[test]
fn board_count_is_zero_without_usb_ports() {
    assert_eq!(count_board_ports(&[network_port()]), 0);
    assert_eq!(count_board_ports(&[]), 0);
}
