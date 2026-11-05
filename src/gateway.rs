use tonic::Code::Ok;

use crate::{
    gateway::{
        GatewayType::{HTTP, TCP},
        http_local::HttpLocalGateway,
        tcp::TcpGateway,
    },
    handler::{
        arduino::{self, ArduinoHandler},
        ble::{self, BleHandler},
        thingblock::{self, ThingblockHandler},
    },
};

mod http_local;
mod tcp;

pub enum GatewayType {
    HTTP,
    TCP,
}

pub struct GatewayContext {
    // internal handlers
    arduino: arduino::ArduinoHandler,
    ble: ble::BleHandler,
    thingblock: thingblock::ThingblockHandler,
}

pub trait Gateway {
    fn serve(&self, context: &GatewayContext);
}

pub struct ThingblockLink {
    context: GatewayContext,
    gateway: Box<dyn Gateway>,
}

impl ThingblockLink {
    fn init(gateway_type: GatewayType) -> Result<Self, Box<dyn std::error::Error>> {
        let context = GatewayContext {
            // todo: init context here
        };

        let gateway: Box<dyn Gateway> = match gateway_type {
            HTTP => Box::new(HttpLocalGateway {}),
            TCP => Box::new(TcpGateway {}),
        };

        Ok(Self {
            gateway: gateway,
            context: context,
        })
    }

    fn serve(&self) {
        self.gateway.serve(&self.context);
    }
}
