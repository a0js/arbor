use std::collections::HashMap;
use config::{Config, Environment};
use serde::Deserialize;

trait Connector {
    fn read_rows (&self);
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectorConfig {
    Csv { file: String },
    Postgres { host: String, port: String, user: String, password: String, database: String },
}

#[derive(Deserialize)]
pub struct ConnectorConfigFile {
    pub connector_configs: HashMap<String, ConnectorConfig>,
}

pub fn load_connector_config() -> Result<ConnectorConfigFile, config::ConfigError> {
    Config::builder()
        .add_source(config::File::with_name("config/connectors"))
        .add_source(config::File::with_name("config/connectors.local"))
        .add_source(Environment::with_prefix("ARROR").separator("__"))
    .build()?
    .try_deserialize()
}

pub struct ConnectorRegistry {
    connectors: HashMap<String, Box<dyn Connector>>,
}

impl ConnectorRegistry {
    pub fn from_config(file: ConnectorConfigFile) -> Result<Self, config::ConfigError> {
        let connectors = file.connector_configs
        .into_iter()
            .map(|(name, cfg)| Ok((name, build_connector(cfg)?)))
        .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(Self { connectors })
    }

    pub fn get(&self, name: &str) -> Option<&dyn Connector> {
        self.connectors.get(name).map(|c| c.as_ref())
    }

}

fn build_connector(cfg: ConnectorConfig) -> Result<Box<dyn Connector>, config::ConfigError> {
    match cfg {
        ConnectorConfig::Csv { file } => Ok(Box::new(CsvConnector::new(file))),
        ConnectorConfig::Postgres { .. } => unimplemented!(),
    }
}

pub struct CsvConnector {
    file_path: String,
}

impl Connector for CsvConnector {
    fn read_rows(&self) {
        todo!()
    }
}

impl CsvConnector {
    pub fn new(file_path: impl Into<String>) -> Self {
        file_path
    }
}

pub struct CsvDataConfig {
    connector: CsvConnector,
    column_map: Option<HashMap<String, String>>,
}

impl CsvDataConfig {
    pub fn new() -> Self {}
}

pub fn load_config(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

}
