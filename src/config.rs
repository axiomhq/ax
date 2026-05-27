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

    /// Pick the deployment to use, with an optional explicit override.
    ///
    /// `override_name` takes precedence over `active_deployments`. It's how
    /// the `--deployment` CLI flag wires in: a launch-time choice beats the
    /// persistent config field. An unknown override is a hard error so
    /// typos surface immediately instead of silently falling back.
    pub fn select(&self, override_name: Option<&str>) -> Result<(&str, &Deployment)> {
        if let Some(name) = override_name.map(str::trim).filter(|s| !s.is_empty()) {
            let (key, dep) = self
                .deployments
                .get_key_value(name)
                .ok_or_else(|| anyhow!("deployment \"{name}\" not found in ~/.axiom.toml"))?;
            return Ok((key.as_str(), dep));
        }

        if let Some(name) = self.active_deployments.as_deref().filter(|s| !s.is_empty()) {
            let (key, dep) = self
                .deployments
                .get_key_value(name)
                .ok_or_else(|| anyhow!("active_deployments=\"{name}\" not found"))?;
            return Ok((key.as_str(), dep));
        }

        if self.deployments.len() == 1 {
            let (name, dep) = self.deployments.iter().next().unwrap();
            return Ok((name.as_str(), dep));
        }

        bail!(
            "multiple deployments configured but no active_deployments set; \
             pass --deployment NAME or set active_deployments in ~/.axiom.toml"
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
mod tests;
