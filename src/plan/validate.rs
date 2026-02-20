use anyhow::{Context, Result};
use spindle_core::pipeline::{PrepareOptions, prepare};
use spindle_parser::parse_spl;

pub fn validate_spl(spl: &str) -> Result<()> {
    let theory = parse_spl(spl).context("SPL parse failed")?;
    prepare(&theory, PrepareOptions::default()).context("SPL prepare/validation failed")?;
    Ok(())
}
