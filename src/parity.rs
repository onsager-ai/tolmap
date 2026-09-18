use std::fmt;
use std::path::Path;

use anyhow::{bail, Result};

pub struct ParityReport;

impl fmt::Display for ParityReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("parity passed")
    }
}

pub fn compare_files(
    _reference: &Path,
    _candidate: &Path,
    _idf_names: Option<&Path>,
) -> Result<ParityReport> {
    bail!("parity harness is not yet initialized")
}

