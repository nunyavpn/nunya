//! Client side of the core IPC protocol.

pub mod codec;
pub mod link;
pub mod peer;

pub use link::{CoreLink, LinkError};

/// Message types generated at build time by `build.rs` from `vendor/core/proto/nunya.proto`,
/// which `scripts/fetch-core.sh` installs from the pinned nunya-core release.
///
/// The proto is proto2, so scalar fields arrive as `Option<T>`; `.unwrap_or_default()` reproduces
/// the `[default = ...]` the Go side assumes.
pub mod gen {
    include!(concat!(env!("OUT_DIR"), "/nunya.rs"));
}

/// Method names exactly as the Go handler table spells them
/// (`handlers` in the core's `internal/rpc/dispatch.go`). A typo here is an "unknown method" error at
/// runtime, so they live in one place.
pub mod method {
    pub const START: &str = "Start";
    pub const STOP: &str = "Stop";
    pub const CHECK_CONFIG: &str = "CheckConfig";
    pub const QUERY_STATS: &str = "QueryStats";
    pub const IS_PRIVILEGED: &str = "IsPrivileged";
    pub const GET_DEFAULT_INTERFACE: &str = "GetDefaultInterface";
    pub const TEST: &str = "Test";
    pub const QUERY_URL_TEST: &str = "QueryURLTest";
    pub const STOP_TEST: &str = "StopTest";
    pub const QUERY_CONNECTIONS: &str = "QueryConnections";
}
