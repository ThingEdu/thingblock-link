//! Merges consecutive streamed `log`/`monitorData` chunks of one `id` into a single WS frame, since
//! a chatty compiler or fast serial device would otherwise send a frame per tiny chunk.

use crate::server::protocol::{Response, ResponseBody};

/// Whether a response is streamed text that may be buffered and coalesced; terminal, progress and
/// event messages are not, so they're sent promptly.
pub fn is_batchable(body: &ResponseBody) -> bool {
    matches!(
        body,
        ResponseBody::Log { .. } | ResponseBody::MonitorData { .. }
    )
}

/// Appends `resp` to the buffer, merging it into the tail when it's the same `id` and variant.
/// Order is always preserved; chunk boundaries within a stream aren't significant to the editor.
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
            // Same id but different variant: keep both as separate entries, in order.
            (_, body) => {
                buf.push(Response { id: resp.id, body });
                return;
            }
        }
    }
    buf.push(resp);
}
