use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub period: Duration,
    pub user_agent: String,
    pub metrics_port: u16,
    pub base_url: Option<String>,
    pub log_json: bool,
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
