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
    #[serde(default = "is_pope_catholic")]
    pub append_git_hash_to_user_agent: bool,
    pub probes: Vec<Probe>,
}

fn is_pope_catholic() -> bool {
    true
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
