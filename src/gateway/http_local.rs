pub(crate) mod model;
mod model_ws;
mod router;

use tower_http::cors::{Any, CorsLayer};

use crate::{error::Result, gateway::GatewayContext};

pub struct HttpLocalGateway {
    port: u16,
}

impl HttpLocalGateway {
    pub(crate) fn new(port: u16) -> Self {
        HttpLocalGateway { port }
    }

    pub(crate) async fn serve(&self, _context: GatewayContext) -> Result<()> {
        let _cors = CorsLayer::new()
            .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
            .allow_methods(Any)
            .allow_headers(Any)
            .allow_private_network(true);
        Ok(())
    }
}
