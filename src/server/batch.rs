//! Outbound coalescing for the writer task. A fast serial device or a chatty
//! compiler streams many tiny `monitorData` / `log` chunks; sending one WS frame
//! each is wasteful on both ends. The writer buffers these streamed-text chunks
//! over a short window (see `BATCH_COOLDOWN` in `session.rs`) and merges
//! consecutive same-`id` chunks into one frame. The wire envelope is unchanged —
//! a payload just carries a longer concatenated string — so chunk boundaries
//! within a stream are not significant to the editor.
//!
//! Only `log` / `monitorData` are batched; terminal (`result`/`error`), `progress`
//! and `event` messages bypass the buffer and are sent promptly.

use crate::server::protocol::{Response, ResponseBody};

/// Whether a response may be buffered/coalesced (streamed text). Terminal,
/// progress and event messages return `false` and are sent promptly.
pub fn is_batchable(body: &ResponseBody) -> bool {
    matches!(
        body,
        ResponseBody::Log { .. } | ResponseBody::MonitorData { .. }
    )
}

/// Append `resp` to the pending buffer, merging into the tail when it is the same
/// `id` and the same streamed-text variant (`log`+`log`, `monitorData`+
/// `monitorData`). Otherwise push it as a new entry. Order is always preserved.
pub fn push_coalesced(buf: &mut Vec<Response>, resp: Response) {
    if let Some(last) = buf.last_mut()
        && last.id == resp.id
    {
        match (&mut last.body, resp.body) {
            (ResponseBody::Log { chunk }, ResponseBody::Log { chunk: more }) => {
                chunk.push_str(&more);
                return;
            }
            (ResponseBody::MonitorData { data }, ResponseBody::MonitorData { data: more }) => {
                data.push_str(&more);
                return;
            }
            // Same id but different variant: keep both, preserving order.
            (_, body) => {
                buf.push(Response { id: resp.id, body });
                return;
            }
        }
    }
    buf.push(resp);
}
