use std::sync::Arc;

use slog::{o, Logger};

use crate::probe::{Endpoint, Probe, ProbeMass};

pub async fn run_loop(
    client: kittycad::Client,
    config: crate::config::Config,
    probes: Vec<crate::probe::Probe>,
    logger: Logger,
) {
    let n = probes.len();
    let client = Arc::new(client);
    loop {
        // Start all probes in parallel
        let mut handles = Vec::with_capacity(n);
        for probe in probes.clone() {
            let probe_client = client.clone();
            let name = probe.name.clone();
            let logger = logger.new(o!("probe_name" => name.clone()));
            let handle = tokio::spawn(async move {
                let _ = run_and_report(probe, probe_client, logger).await;
            });
            handles.push((name, handle));
        }

        // Wait for all probes.
        for (name, handle) in handles {
            if let Err(e) = handle.await {
                slog::error!(logger, "probe panicked"; "probe_name" => name, "panic" => ?e);
            }
        }

        slog::info!(logger, "sleeping"; "duration" => config.period.as_secs());
        tokio::time::sleep(config.period).await;
    }
}

/// Runs the probe and reports its results (via logs and metrics).
async fn run_and_report(probe: Probe, client: Arc<kittycad::Client>, logger: Logger) {
    let result = probe_endpoint(probe, client).await;
    match result {
        Ok(()) => {
            slog::info!(logger, "probe succeeded");
        }
        Err(err) => {
            slog::error!(logger, "probe failed"; "error" => format!("{}", err));
        }
    }
}

/// Probe the specified API endpoint, check it returns the expected response.
/// The probe could fail because the API is unavailable, or because it gave an unexpected result.
/// If this returns OK, the endpoint is "healthy". Otherwise there's a problem.
pub async fn probe_endpoint(probe: Probe, client: Arc<kittycad::Client>) -> Result<(), Error> {
    match probe.endpoint {
        Endpoint::FileMass { file_path, probe } => {
            let file = tokio::fs::read(file_path).await.unwrap();
            probe_file_mass(file, probe, client).await
        }
        Endpoint::Ping => {
            let _pong = client.meta().ping().await?;
            Ok(())
        }
    }
}

#[autometrics::autometrics]
async fn probe_file_mass(
    file: Vec<u8>,
    ProbeMass {
        src_format,
        material_density,
        material_density_unit,
        mass_unit,
        expected,
    }: ProbeMass,
    client: Arc<kittycad::Client>,
) -> Result<(), Error> {
    let resp = client
        .file()
        .create_mass(
            material_density,
            material_density_unit,
            mass_unit,
            src_format,
            &bytes::Bytes::from(file),
        )
        .await?;
    if expected.matches_actual(&resp) {
        Ok(())
    } else {
        Err(Error::UnexpectedApiResponse {
            expected: format!("{expected:?}"),
            actual: format!("{resp:?}"),
        })
    }
}

/// Errors the probe can detect.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("KittyCAD API client returned an error: {0}")]
    ApiClient(#[from] kittycad::types::error::Error),
    #[error("KittyCAD API sent {actual} but probe expected {expected}")]
    UnexpectedApiResponse { expected: String, actual: String },
}
