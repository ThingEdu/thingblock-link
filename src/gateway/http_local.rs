pub(crate) mod model;
mod model_ws;
mod router;

use tower_http::cors::{Any, CorsLayer};

use crate::gateway::Gateway;

pub struct HttpLocalGateway {}

impl HttpLocalGateway {
    fn new() -> Self {
        HttpLocalGateway {}
    }
}

impl Gateway for HttpLocalGateway {
    fn serve(context: super::GatewayContext) {
        let cors = CorsLayer::new()
            .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
            .allow_methods(Any)
            .allow_headers(Any)
            .allow_private_network(true);
    }
}
