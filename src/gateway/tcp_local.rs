use crate::{error::Result, gateway::GatewayContext};

pub struct TcpGateway {
    port: u16,
}

impl TcpGateway {
    pub(crate) fn new(port: u16) -> Self {
        TcpGateway { port }
    }

    pub(crate) async fn serve(&self, _context: GatewayContext) -> Result<()> {
        Ok(())
    }
}
