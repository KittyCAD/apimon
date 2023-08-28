use std::time::Duration;

use reqwest::Url;
use serde::Deserialize;

use crate::probe::Probe;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub period: Duration,
    pub user_agent: String,
    pub metrics_addr: std::net::SocketAddr,
    pub base_url: Option<Url>,
    pub log_json: bool,
    pub probes: Vec<Probe>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_config() {
        let config = include_bytes!("../configuration/config.yaml");
        let _config: Config = serde_yaml::from_slice(config).unwrap();
    }
}
