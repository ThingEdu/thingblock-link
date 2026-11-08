//! Core of the local tray helper (`thingblock-link`): a translating proxy from the WS envelope
//! to arduino-cli gRPC.

// Scaffolding: some modules are milestone stubs whose items aren't used yet. Remove this once
// every component is wired together and its items are actually used.
#![allow(dead_code)]

pub mod error;
pub mod utils;

pub mod gateway;
pub mod service;

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::{
    error::Result,
    gateway::{Gateway, GatewayContext, GatewayType},
    service::{
        arduino::ArduinoService,
        ble::BleService,
        thingblock::{ThingblockService, resource::ResourceRoot},
    },
};

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

        Ok(Self {
            context: GatewayContext {
                arduino,
                ble,
                thingblock,
            },
            gateway: Gateway::new(gateway_type),
        })
    }

    pub fn arduino(&self) -> Arc<ArduinoService> {
        self.context.arduino.clone()
    }

    pub async fn serve(&self) -> Result<()> {
        self.gateway.serve(self.context.clone()).await
    }
}
