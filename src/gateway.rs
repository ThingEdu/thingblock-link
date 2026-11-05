use crate::{
    gateway::{http_local::HttpLocalGateway, tcp::TcpGateway},
    handler::{arduino, ble, thingblock},
};

mod http_local;
mod tcp;

pub struct GatewayContext {
    // internal handlers
    arduino: arduino::ArduinoHandler,
    ble: ble::BleHandler,
    thingblock: thingblock::ThingblockHandler,
}

pub trait Gateway {
    fn serve(context: GatewayContext);
}

pub struct ThingblockLink {}

impl ThingblockLink {
    pub fn init_http() -> HttpLocalGateway {
        HttpLocalGateway {}
    }

    pub fn init_tcp() -> TcpGateway {
        TcpGateway {}
    }
}
