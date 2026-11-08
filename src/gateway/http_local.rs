mod handler;
mod handler_ws;
mod model;
mod model_ws;
mod router;

use std::net::Ipv4Addr;

use tokio::net::TcpListener;
use tracing::info;

use crate::error::{Error, Result};
use crate::gateway::GatewayContext;

pub struct HttpLocalGateway {
    port: u16,
}

impl HttpLocalGateway {
    pub fn new(port: u16) -> Self {
        HttpLocalGateway { port }
    }

    /// Binds `127.0.0.1:<port>` and serves the HTTP routes until the server stops.
    pub async fn serve(&self, context: GatewayContext) -> Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.port))
            .await
            .map_err(|e| Error::Gateway(format!("bind 127.0.0.1:{}: {e}", self.port)))?;
        info!(port = self.port, "http gateway listening");

        axum::serve(listener, router::router(context))
            .await
            .map_err(|e| Error::Gateway(format!("serve on port {}: {e}", self.port)))
    }
}
