use crate::gateway::{self, Gateway};

pub struct TcpGateway {}

impl TcpGateway {
    fn new() -> Self {
        TcpGateway {}
    }
}

impl Gateway for TcpGateway {
    fn serve(context: super::GatewayContext) {}
}
