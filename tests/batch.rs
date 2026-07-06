//! Unit coverage for the writer's outbound coalescing (`ws::batch`). Pure logic,
//! no daemon — the 100 ms flush timing in the writer task is exercised by manual
//! end-to-end runs (see the design doc). Kept under `tests/` per the project rule.

use thingblock_link::server::batch::{is_batchable, push_coalesced};
use thingblock_link::server::protocol::{Response, ResponseBody};

fn log(id: &str, chunk: &str) -> Response {
    Response {
        id: id.into(),
        body: ResponseBody::Log {
            chunk: chunk.into(),
        },
    }
}

fn monitor(id: &str, data: &str) -> Response {
    Response {
        id: id.into(),
        body: ResponseBody::MonitorData { data: data.into() },
    }
}

#[test]
fn only_streamed_text_is_batchable() {
    assert!(is_batchable(&ResponseBody::Log { chunk: "x".into() }));
    assert!(is_batchable(&ResponseBody::MonitorData {
        data: "x".into()
    }));

    assert!(!is_batchable(&ResponseBody::Progress {
        phase: "link".into(),
        percent: 50.0,
    }));
    assert!(!is_batchable(&ResponseBody::Result(serde_json::json!({}))));
    assert!(!is_batchable(&ResponseBody::Error {
        code: "boom".into(),
        message: "boom".into(),
    }));
    assert!(!is_batchable(&ResponseBody::Event(serde_json::json!({}))));
}

#[test]
fn merges_consecutive_same_id_log_chunks() {
    let mut buf = Vec::new();
    push_coalesced(&mut buf, log("7", "He"));
    push_coalesced(&mut buf, log("7", "ll"));
    push_coalesced(&mut buf, log("7", "o"));

    assert_eq!(buf.len(), 1);
    assert_eq!(buf[0].id, "7");
    match &buf[0].body {
        ResponseBody::Log { chunk } => assert_eq!(chunk, "Hello"),
        other => panic!("expected log, got {other:?}"),
    }
}

#[test]
fn merges_consecutive_same_id_monitor_data() {
    let mut buf = Vec::new();
    push_coalesced(&mut buf, monitor("m", "ab"));
    push_coalesced(&mut buf, monitor("m", "cd"));

    assert_eq!(buf.len(), 1);
    match &buf[0].body {
        ResponseBody::MonitorData { data } => assert_eq!(data, "abcd"),
        other => panic!("expected monitorData, got {other:?}"),
    }
}

#[test]
fn does_not_merge_across_ids() {
    let mut buf = Vec::new();
    push_coalesced(&mut buf, log("a", "1"));
    push_coalesced(&mut buf, log("b", "2"));

    assert_eq!(buf.len(), 2);
    assert_eq!(buf[0].id, "a");
    assert_eq!(buf[1].id, "b");
}

#[test]
fn does_not_merge_different_variants_same_id() {
    let mut buf = Vec::new();
    push_coalesced(&mut buf, log("x", "1"));
    push_coalesced(&mut buf, monitor("x", "2"));

    assert_eq!(buf.len(), 2);
    assert!(matches!(buf[0].body, ResponseBody::Log { .. }));
    assert!(matches!(buf[1].body, ResponseBody::MonitorData { .. }));
}

#[test]
fn interleaved_ids_keep_order_without_merging() {
    let mut buf = Vec::new();
    push_coalesced(&mut buf, log("a", "1"));
    push_coalesced(&mut buf, log("b", "2"));
    push_coalesced(&mut buf, log("a", "3"));

    let ids: Vec<&str> = buf.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "a"]);
}
