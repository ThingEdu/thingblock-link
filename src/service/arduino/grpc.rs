//! Thin wrapper over the generated arduino-cli gRPC client, with one submodule per RPC.
//! Keeps the arduino-cli schema contained: nothing past this module sees its types.

use tonic::transport::Channel;

pub mod board;
pub mod compile;
pub mod monitor;
pub mod platform;
pub mod upload;

/// The tonic code `build.rs` generates (protox + tonic into `OUT_DIR`), mirroring the proto
/// packages. Lints are silenced because generated code is not ours to clean up.
#[allow(clippy::all, clippy::pedantic)]
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/mod.rs"));
}

pub use pb::cc::arduino::cli::commands::v1 as cli;

use cli::arduino_core_service_client::ArduinoCoreServiceClient;

/// gRPC client bound to one initialized daemon instance. The `Channel` is a cheap handle to a
/// shared connection pool; the `Instance` from `Create`/`Init` is required by every RPC.
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
