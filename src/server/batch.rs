//! Coalesces streamed `log`/`monitorData` chunks per `id`; chunk boundaries aren't significant to the editor.

use crate::server::protocol::{Response, ResponseBody};

pub fn is_batchable(body: &ResponseBody) -> bool {
    matches!(
        body,
        ResponseBody::Log { .. } | ResponseBody::MonitorData { .. }
    )
}

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
            (_, body) => {
                buf.push(Response { id: resp.id, body });
                return;
            }
        }
    }
    buf.push(resp);
}
