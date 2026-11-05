pub struct WsRequest {
    id: u16,
    body: WsRequestBody,
}

pub enum WsRequestBody {}

pub struct WsResponse {
    id: u16,
    body: WsResponseBody,
}

pub enum WsResponseBody {}
