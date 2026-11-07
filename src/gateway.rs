mod http_cloud;
mod http_local;
mod tcp_local;

use std::sync::Arc;

use crate::{
    error::Result,
    gateway::{http_local::HttpLocalGateway, tcp_local::TcpGateway},
    service::{arduino::ArduinoService, ble::BleService, thingblock::ThingblockService},
};

pub enum GatewayType {
    Http { port: u16 },
    Tcp { port: u16 },
}

#[derive(Clone)]
pub(crate) struct GatewayContext {
    pub(crate) arduino: Arc<ArduinoService>,
    pub(crate) ble: Option<Arc<BleService>>,
    pub(crate) thingblock: Arc<ThingblockService>,
}

pub(crate) enum Gateway {
    Http(HttpLocalGateway),
    Tcp(TcpGateway),
}

impl Gateway {
    pub(crate) fn new(gateway_type: GatewayType) -> Self {
        match gateway_type {
            GatewayType::Http { port } => Self::Http(HttpLocalGateway::new(port)),
            GatewayType::Tcp { port } => Self::Tcp(TcpGateway::new(port)),
        }
    }

    pub(crate) async fn serve(&self, context: GatewayContext) -> Result<()> {
        match self {
            Self::Http(gateway) => gateway.serve(context).await,
            Self::Tcp(gateway) => gateway.serve(context).await,
        }
    }
}
