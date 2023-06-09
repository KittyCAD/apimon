use std::sync::Arc;

use camino::Utf8PathBuf;
use slog::Logger;

use crate::probe::{Endpoint, Probe};

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
            let handle = tokio::spawn(async move {
                let probe = probe.clone();
                run_probe(&probe, probe_client).await
            });
            handles.push((handle, name));
        }

        // Collect all probes
        let mut results = Vec::with_capacity(n);
        for (handle, name) in handles {
            results.push((name, handle.await));
        }

        // Handle results
        for (probe_name, result) in results {
            let logger = logger.new(slog::o!("probe_name" => probe_name));
            match result {
                Ok(Ok(())) => {
                    slog::info!(logger, "probe succeeded");
                }
                Ok(Err(err)) => {
                    slog::error!(logger, "probe failed"; "error" => format!("{}", err));
                }
                Err(err) => {
                    slog::error!(logger, "probe panicked"; "panic_error" => format!("{:?}", err));
                }
            }
        }

        slog::info!(logger, "sleeping"; "duration" => config.period.as_secs());
        tokio::time::sleep(config.period).await;
    }
}

pub async fn run_probe(probe: &Probe, client: Arc<kittycad::Client>) -> Result<(), Error> {
    match &probe.endpoint {
        Endpoint::FileMass {
            file_path,
            src_format,
            material_density,
            expected,
        } => {
            let file = tokio::fs::read(file_path)
                .await
                .map_err(|err| Error::BadInputFile {
                    file_path: file_path.clone(),
                    err,
                })?;
            let resp = client
                .file()
                .create_mass(
                    *material_density,
                    src_format.clone(),
                    &bytes::Bytes::from(file),
                )
                .await
                .map_err(Error::ApiClient)?;
            if expected.matches_actual(&resp) {
                Ok(())
            } else {
                Err(Error::UnexpectedApiResponse {
                    expected: format!("{expected:?}"),
                    actual: format!("{resp:?}"),
                })
            }
        }
    }
}

/// Errors the probe can detect.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Could not read input file {file_path}: {err}")]
    BadInputFile {
        file_path: Utf8PathBuf,
        err: std::io::Error,
    },
    #[error("KittyCAD API client returned an error: {0}")]
    ApiClient(kittycad::types::error::Error),
    #[error("KittyCAD API sent {actual} but probe expected {expected}")]
    UnexpectedApiResponse { expected: String, actual: String },
}
