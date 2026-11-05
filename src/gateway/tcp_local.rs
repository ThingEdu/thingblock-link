use crate::{error::Result, gateway::GatewayContext};

pub struct TcpGateway {
    port: u16,
}

impl TcpGateway {
    pub fn new(port: u16) -> Self {
        TcpGateway { port }
    }

    pub async fn serve(&self, _context: GatewayContext) -> Result<()> {
        Ok(())
    }
}
