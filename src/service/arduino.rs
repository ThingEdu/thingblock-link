//! The arduino-cli backend: process lifecycle ([`daemon`]), the generated
//! gRPC client ([`grpc`]), and the WS-envelope translator ([`bridge`]) that
//! ties them to the flash channel ([`crate::server`]).

pub mod bridge;
pub mod daemon;
pub mod grpc;
