use run_loop::run_loop;
use slog::{info, Logger};
use std::fs::read_to_string;

mod config;
mod metrics;
mod probe;
mod run_loop;

const CONFIG_PATH: &str = "configuration/config.yaml";
const PROBES_PATH: &str = "configuration/probes.yaml";

#[tokio::main]
async fn main() {
    // Read all necessary configuration and data
    let config = read_config();
    let logger = new_logger(config.log_json);
    info!(logger, "starting"; "config" => ?config);

    let probes = read_probes();
    let api_token = std::env::var("KITTYCAD_API_TOKEN")
        .unwrap_or_else(|e| terminate(&format!("Failed to read KITTYCAD_API_TOKEN: {e}")));

    // Start the metrics server
    tokio::task::spawn(metrics::serve(config.metrics_addr, logger.clone()));

    // Run the probes
    let mut client = kittycad::Client::new(api_token);
    if let Some(ref url) = config.base_url {
        client.set_base_url(url);
    }
    run_loop(client, config, probes, logger).await;
}

/// Terminates if data is missing or invalid.
fn read_config() -> config::Config {
    let config_file = read_to_string(CONFIG_PATH)
        .unwrap_or_else(|e| terminate(&format!("Failed to read {CONFIG_PATH}: {e}")));
    serde_yaml::from_str(&config_file)
        .unwrap_or_else(|e| terminate(&format!("Failed to parse {CONFIG_PATH}: {e}")))
}

/// Terminates if data is missing or invalid.
fn read_probes() -> Vec<probe::Probe> {
    let probes_file = read_to_string(PROBES_PATH)
        .unwrap_or_else(|e| terminate(&format!("Failed to read {PROBES_PATH}: {e}")));
    serde_yaml::from_str(&probes_file)
        .unwrap_or_else(|e| terminate(&format!("Failed to parse {PROBES_PATH}: {e}")))
}

fn terminate(why: &str) -> ! {
    eprintln!("{why}");
    std::process::exit(1);
}

/// Create a colourful, terminal-based logger for local dev,
/// or a JSON logger for production.
fn new_logger(json: bool) -> Logger {
    use slog::Drain;
    use slog_async::Async;
    if json {
        let drain = slog_json::Json::default(std::io::stderr()).fuse();
        let async_drain = Async::new(drain).build().fuse();
        Logger::root(async_drain, slog::o!())
    } else {
        let decorator = slog_term::TermDecorator::new().build();
        let drain = slog_term::FullFormat::new(decorator).build().fuse();
        let async_drain = Async::new(drain).build().fuse();
        Logger::root(async_drain, slog::o!())
    }
}
