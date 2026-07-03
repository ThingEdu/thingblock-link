//! Generated `ArduinoCoreService` client and the thin wrapper the rest of the
//! helper talks to.
//!
//! `build.rs` runs `protox` over the vendored protos in `proto/` and feeds the
//! descriptor set to tonic, which emits one nested module tree (`mod.rs`) into
//! `OUT_DIR`. [`pb`] re-exports it; [`cli`] is the convenient alias for the
//! `cc.arduino.cli.commands.v1` package.
//!
//! Per-RPC helper methods (`board_list`, `compile`, …) that translate to the
//! helper's shapes land on [`Client`] via one submodule per RPC (each a separate
//! `impl Client` block, e.g. [`board`]); the arduino-cli schema never leaks past
//! this module.

use tonic::transport::Channel;

pub mod board;
pub mod compile;
pub mod monitor;
pub mod platform;
pub mod upload;

/// The tonic-generated code, mirroring the proto package hierarchy. Lints are
/// silenced here — this is machine-generated and not ours to clean up.
#[allow(clippy::all, clippy::pedantic)]
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/mod.rs"));
}

/// Alias for the arduino-cli commands package.
pub use pb::cc::arduino::cli::commands::v1 as cli;

use cli::arduino_core_service_client::ArduinoCoreServiceClient;

/// gRPC client bound to one initialized daemon instance.
///
/// Holds a `Channel` (cheap to clone — it is a handle to the shared connection
/// pool) and the `Instance` returned by `Create`/`Init`, which every RPC needs.
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

    /// The raw generated client, for milestones that add RPC wrappers here.
    pub fn inner(&mut self) -> &mut ArduinoCoreServiceClient<Channel> {
        &mut self.inner
    }

    pub fn instance(&self) -> &cli::Instance {
        &self.instance
    }
}
