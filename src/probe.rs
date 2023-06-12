use camino::Utf8PathBuf;
use kittycad::types::{FileImportFormat, FileMass};
use serde::Deserialize;
use std::string::ToString;

// f64s are inherently approximate, so we need to define a tolerance for equality checks.
const EPSILON: f64 = 0.000_000_1;

/// A probe that this API monitor should run.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct Probe {
    pub name: String,
    pub endpoint: Endpoint,
}

/// A KittyCAD API endpoint that this API monitor can probe.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum Endpoint {
    FileMass {
        file_path: Utf8PathBuf,
        src_format: FileImportFormat,
        material_density: f64,
        expected: ExpectedFileMass,
    },
    Ping,
}

/// Properties of `kittycad::types::FileMass` to validate.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct ExpectedFileMass {
    mass: Option<f64>,
    status: kittycad::types::ApiCallStatus,
    error: Option<String>,
    src_format: FileImportFormat,
}

impl ExpectedFileMass {
    pub fn matches_actual(&self, actual: &FileMass) -> bool {
        let mass_same = match (self.mass, actual.mass) {
            (None, None) => true,
            (Some(a), Some(b)) => (a - b).abs() < EPSILON,
            _ => false,
        };
        mass_same
            && self.status == actual.status
            && self.error == actual.error.as_ref().map(ToString::to_string)
            && self.src_format == actual.src_format
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_yaml() {
        let data = include_bytes!("../configuration/probes.yaml");
        let _data: Vec<Probe> = serde_yaml::from_slice(data).unwrap();
    }
}
