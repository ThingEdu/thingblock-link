//! Round-trip tests for the WS `{id, type, payload}` envelope (the cross-repo
//! contract). Kept here rather than inline in `src/` so source files stay focused
//! on implementation.

use thingblock_link::server::protocol::{
    ListBoardsResult, Request, RequestBody, Response, ResponseBody,
};

#[test]
fn deserializes_list_boards_request() {
    let raw = r#"{"id":"1","type":"listBoards","payload":{"pnpid":["2341:0043"]}}"#;
    let req: Request = serde_json::from_str(raw).expect("parse listBoards");
    assert_eq!(req.id, "1");
    match req.body {
        RequestBody::ListBoards { pnpid } => assert_eq!(pnpid, ["2341:0043"]),
        other => panic!("unexpected body: {other:?}"),
    }
}

#[test]
fn deserializes_upload_request_with_camelcase_fields() {
    let raw = r#"{
        "id":"7","type":"upload",
        "payload":{"fqbn":"arduino:avr:uno","port":"/dev/ttyACM0",
                   "uploadSpeed":115200,
                   "artifact":{"format":"hex","path":"/tmp/sketch.hex"}}
    }"#;
    let req: Request = serde_json::from_str(raw).expect("parse upload");
    match req.body {
        RequestBody::Upload {
            upload_speed,
            artifact,
            ..
        } => {
            assert_eq!(upload_speed, 115200);
            assert_eq!(artifact.format, "hex");
        }
        other => panic!("unexpected body: {other:?}"),
    }
}

#[test]
fn deserializes_compile_request_with_lib_refs() {
    let raw = r#"{
        "id":"3","type":"compile",
        "payload":{"fqbn":"arduino:avr:uno","options":{},"source":"void setup(){}",
                   "libs":[{"pack":"dht","lib":"lib/DHT"}]}
    }"#;
    let req: Request = serde_json::from_str(raw).expect("parse compile");
    match req.body {
        RequestBody::Compile { libs, .. } => {
            assert_eq!(libs.len(), 1);
            assert_eq!(libs[0].pack, "dht");
            assert_eq!(libs[0].lib, "lib/DHT");
        }
        other => panic!("unexpected body: {other:?}"),
    }
}

#[test]
fn deserializes_compile_request_without_libs_defaults_empty() {
    let raw = r#"{
        "id":"4","type":"compile",
        "payload":{"fqbn":"arduino:avr:uno","options":{},"source":"void setup(){}"}
    }"#;
    let req: Request = serde_json::from_str(raw).expect("parse compile without libs");
    match req.body {
        RequestBody::Compile { libs, .. } => assert!(libs.is_empty()),
        other => panic!("unexpected body: {other:?}"),
    }
}

#[test]
fn deserializes_monitor_open_request_with_camelcase_fields() {
    let raw =
        r#"{"id":"9","type":"monitorOpen","payload":{"port":"/dev/ttyACM0","baudRate":115200}}"#;
    let req: Request = serde_json::from_str(raw).expect("parse monitorOpen");
    match req.body {
        RequestBody::MonitorOpen { port, baud_rate } => {
            assert_eq!(port, "/dev/ttyACM0");
            assert_eq!(baud_rate, 115200);
        }
        other => panic!("unexpected body: {other:?}"),
    }
}

#[test]
fn deserializes_monitor_write_request() {
    let raw = r#"{"id":"10","type":"monitorWrite","payload":{"data":"hello"}}"#;
    let req: Request = serde_json::from_str(raw).expect("parse monitorWrite");
    match req.body {
        RequestBody::MonitorWrite { data } => assert_eq!(data, "hello"),
        other => panic!("unexpected body: {other:?}"),
    }
}

#[test]
fn deserializes_install_platform_request() {
    let raw = r#"{"id":"5","type":"installPlatform","payload":{"platform":"esp32:esp32"}}"#;
    let req: Request = serde_json::from_str(raw).expect("parse installPlatform");
    match req.body {
        RequestBody::InstallPlatform { platform, version } => {
            assert_eq!(platform, "esp32:esp32");
            assert_eq!(version, None, "version defaults to latest");
        }
        other => panic!("unexpected body: {other:?}"),
    }
}

#[test]
fn deserializes_install_platform_request_with_version() {
    let raw = r#"{
        "id":"6","type":"installPlatform",
        "payload":{"platform":"esp32:esp32","version":"3.0.5"}
    }"#;
    let req: Request = serde_json::from_str(raw).expect("parse installPlatform with version");
    match req.body {
        RequestBody::InstallPlatform { version, .. } => {
            assert_eq!(version.as_deref(), Some("3.0.5"));
        }
        other => panic!("unexpected body: {other:?}"),
    }
}

#[test]
fn install_platform_request_requires_platform() {
    let raw = r#"{"id":"7","type":"installPlatform","payload":{}}"#;
    assert!(
        serde_json::from_str::<Request>(raw).is_err(),
        "a payload without `platform` must be rejected"
    );
}

#[test]
fn serializes_monitor_data_envelope() {
    let resp = Response {
        id: "9".into(),
        body: ResponseBody::MonitorData {
            data: "42\n".into(),
        },
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["id"], "9");
    assert_eq!(json["type"], "monitorData");
    assert_eq!(json["payload"]["data"], "42\n");
}

#[test]
fn serializes_result_envelope() {
    let resp = Response {
        id: "1".into(),
        body: ResponseBody::Result(
            serde_json::to_value(ListBoardsResult { targets: vec![] }).unwrap(),
        ),
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["id"], "1");
    assert_eq!(json["type"], "result");
    assert!(json["payload"]["targets"].is_array());
}
