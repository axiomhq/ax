//! Load `~/.axiom.toml`.
//!
//! Expected shape (matches the axiom CLI):
//!
//! ```toml
//! active_deployments = "prod"   # optional
//!
//! [deployments.prod]
//! url = "https://api.axiom.co"
//! token = "xaat-..."
//! org_id = "..."
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Deployment {
    pub url: String,
    pub token: String,
    #[serde(default)]
    pub org_id: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub active_deployments: Option<String>,
    #[serde(default)]
    pub deployments: HashMap<String, Deployment>,
}

impl Config {
    /// Load from `~/.axiom.toml`. Returns a clear error if the file is missing or malformed.
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(text).context("parsing axiom config")?;
        if cfg.deployments.is_empty() {
            bail!("no [deployments.*] entries in axiom config");
        }
        Ok(cfg)
    }

    /// Pick the deployment to use. Honors `active_deployments` when present.
    /// Falls back to a single deployment, or errors when ambiguous.
    pub fn active(&self) -> Result<(&str, &Deployment)> {
        if let Some(name) = self.active_deployments.as_deref().filter(|s| !s.is_empty()) {
            let dep = self
                .deployments
                .get(name)
                .ok_or_else(|| anyhow!("active_deployments=\"{name}\" not found"))?;
            return Ok((name, dep));
        }

        if self.deployments.len() == 1 {
            let (name, dep) = self.deployments.iter().next().unwrap();
            return Ok((name.as_str(), dep));
        }

        bail!(
            "multiple deployments configured but no active_deployments set; \
             pick one in ~/.axiom.toml"
        )
    }
}

fn config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    let mut path = PathBuf::from(home);
    path.push(".axiom.toml");
    Ok(path)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
