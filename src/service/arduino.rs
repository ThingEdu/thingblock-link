//! The arduino-cli backend: daemon process lifecycle ([`daemon`]), the gRPC client ([`grpc`]),
//! and the WS-envelope translator ([`bridge`]) that ties them to the WS server.

pub mod bridge;
pub mod daemon;
pub mod grpc;
