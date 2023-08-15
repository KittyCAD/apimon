use camino::Utf8Path;
use config::Config;
use hyper::header::{self, CACHE_CONTROL};
use run_loop::run_loop;
use slog::{info, Logger};
use std::{fs::read_to_string, time::Duration};

mod args;
mod config;
mod metrics;
mod probe;
mod run_loop;

#[tokio::main]
async fn main() {
    // Read all necessary configuration and data
    let args = args::parse_args().unwrap_or_else(|e| terminate(&e.to_string()));
    let config = read_config(&args.config_file);
    let logger = new_logger(config.log_json);
    info!(logger, "starting"; "config" => ?config);

    let probes = read_probes(&args.probes_file);
    let api_token = std::env::var("KITTYCAD_API_TOKEN")
        .unwrap_or_else(|e| terminate(&format!("Failed to read KITTYCAD_API_TOKEN: {e}")));

    // Start the metrics server
    tokio::task::spawn(metrics::serve(config.metrics_addr, logger.clone()));

    // Run the probe
    run_loop(make_client(&config, api_token), config, probes, logger).await;
}

/// Terminates if data is missing or invalid.
fn read_config(config_path: &Utf8Path) -> config::Config {
    let config_file = read_to_string(config_path)
        .unwrap_or_else(|e| terminate(&format!("Failed to read {config_path}: {e}")));
    serde_yaml::from_str(&config_file)
        .unwrap_or_else(|e| terminate(&format!("Failed to parse {config_path}: {e}")))
}

/// Terminates if data is missing or invalid.
fn read_probes(probes_path: &Utf8Path) -> Vec<probe::Probe> {
    let probes_file = read_to_string(probes_path)
        .unwrap_or_else(|e| terminate(&format!("Failed to read {probes_path}: {e}")));
    serde_yaml::from_str(&probes_file)
        .unwrap_or_else(|e| terminate(&format!("Failed to parse {probes_path}: {e}")))
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

fn make_client(config: &Config, api_token: String) -> kittycad::Client {
    let user_agent = config.user_agent.to_owned();
    let base_client = || {
        let mut headers = header::HeaderMap::new();
        headers.insert(CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
        reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(user_agent.clone())
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(5))
    };
    let http = base_client();
    let websocket = base_client().http1_only();
    let mut client = kittycad::Client::new_from_reqwest(api_token, http, websocket);
    if let Some(ref url) = config.base_url {
        client.set_base_url(url);
    }
    client
}
