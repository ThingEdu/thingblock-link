mod http_cloud;
mod http_local;
mod tcp_local;

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::{
    error::Result,
    gateway::{http_local::HttpLocalGateway, tcp_local::TcpGateway},
    service::{
        arduino::ArduinoService,
        ble::BleService,
        thingblock::{ThingblockService, resource::ResourceRoot},
    },
};

pub enum GatewayType {
    Http { port: u16 },
    Tcp { port: u16 },
}

#[derive(Clone)]
struct GatewayContext {
    arduino: Arc<ArduinoService>,
    ble: Option<Arc<BleService>>,
    thingblock: Arc<ThingblockService>,
}

enum Gateway {
    Http(HttpLocalGateway),
    Tcp(TcpGateway),
}

/// Configuration for ThingblockLink,
///
/// `resource_root`: path to `thingblock-resource` directory
/// `arduino_cli`: optional path to the Arduino CLI executable
/// `config_dir`: path to the directory containing the arduino-cli.yaml config file
pub struct LinkConfig {
    pub resource_root: PathBuf,
    pub arduino_cli: Option<PathBuf>,
    pub config_dir: PathBuf,
}
pub struct ThingblockLink {
    context: GatewayContext,
    gateway: Gateway,
}

impl ThingblockLink {
    pub async fn new(gateway_type: GatewayType, config: LinkConfig) -> Result<Self> {
        let thingblock = Arc::new(ThingblockService::new(ResourceRoot::new(
            &config.resource_root,
        )?));

        let arduino = Arc::new(
            ArduinoService::start_with(config.arduino_cli, Some(config.config_dir)).await?,
        );

        // only init BleService if ble adapter is available, otherwise None
        let ble = match BleService::discover().await {
            Ok(Some(ble)) => Some(Arc::new(ble)),
            Ok(None) => {
                info!("no BLE adapter found; BLE disabled");
                None
            }
            Err(e) => {
                warn!(error = %e, "BLE adapter unavailable; BLE disabled");
                None
            }
        };

        let gateway = match gateway_type {
            GatewayType::Http { port } => Gateway::Http(HttpLocalGateway::new(port)),
            GatewayType::Tcp { port } => Gateway::Tcp(TcpGateway::new(port)),
        };

        Ok(Self {
            context: GatewayContext {
                arduino,
                ble,
                thingblock,
            },
            gateway,
        })
    }

    pub fn arduino(&self) -> Arc<ArduinoService> {
        self.context.arduino.clone()
    }

    pub async fn serve(&self) -> Result<()> {
        let context = self.context.clone();
        match &self.gateway {
            Gateway::Http(gateway) => gateway.serve(context).await,
            Gateway::Tcp(gateway) => gateway.serve(context).await,
        }
    }
}
