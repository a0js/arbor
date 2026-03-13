use std::net::SocketAddr;
use std::path::PathBuf;
use serde::Deserialize;
use config::{Config, ConfigError, Environment, File};

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Uds,
    Tcp,
    Both,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthorizerConfig {
    pub snapshot_path: PathBuf,
    #[serde(default = "default_uds_path")]
    pub uds_path: PathBuf,
    #[serde(default = "default_grpc_addr")]
    pub grpc_addr: SocketAddr,
    #[serde(default = "default_transport")]
    pub transport: Transport,
    #[serde(default = "default_max_concurrent_streams")]
    pub max_concurrent_streams: u32,
}

fn default_uds_path() -> PathBuf {
    "/tmp/arbor.sock".into()
}

fn default_grpc_addr() -> SocketAddr {
    "[::1]:50051".parse().unwrap()
}

fn default_transport() -> Transport {
    Transport::Both
}

fn default_max_concurrent_streams() -> u32 {
    1000
}

impl AuthorizerConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let run_mode = std::env::var("RUN_MODE").unwrap_or_else(|_| "development".into());

        let s = Config::builder()
            // Start with default values from a file (optional)
            .add_source(File::with_name("config/authorizer").required(false))
            // Add environment-specific overrides (e.g. config/authorizer.production.toml)
            .add_source(File::with_name(&format!("config/authorizer.{}", run_mode)).required(false))
            // Add local overrides (not committed to git)
            .add_source(File::with_name("config/authorizer.local").required(false))
            // Finally, override with environment variables (prefixed with ARBOR_)
            // e.g. ARBOR_SNAPSHOT_PATH=/data/snapshot.arbor
            .add_source(Environment::with_prefix("ARBOR").separator("_"))
            .build()?;

        s.try_deserialize()
    }
}
