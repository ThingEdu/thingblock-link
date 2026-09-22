//! Wrapper over the tonic `ArduinoCoreService` client; the arduino-cli schema never leaks past here.

use tonic::transport::Channel;

pub mod board;
pub mod compile;
pub mod monitor;
pub mod platform;
pub mod upload;

// Generated code (build.rs: protox + tonic into OUT_DIR); not ours to lint.
#[allow(clippy::all, clippy::pedantic)]
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/mod.rs"));
}

pub use pb::cc::arduino::cli::commands::v1 as cli;

use cli::arduino_core_service_client::ArduinoCoreServiceClient;

pub struct Client {
    inner: ArduinoCoreServiceClient<Channel>,
    instance: cli::Instance,
}

impl Client {
    pub fn new(channel: Channel, instance: cli::Instance) -> Self {
        Self {
            inner: ArduinoCoreServiceClient::new(channel),
            instance,
        }
    }

    pub fn inner(&mut self) -> &mut ArduinoCoreServiceClient<Channel> {
        &mut self.inner
    }

    pub fn instance(&self) -> &cli::Instance {
        &self.instance
    }
}
