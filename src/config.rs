use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

/// Configuration for a single Tekton component (e.g., pipeline, triggers).
#[derive(Debug, Deserialize)]
pub struct ComponentConfig {
    pub repo: String,
    /// Import paths for ko build (e.g. ["./cmd/controller", "./cmd/webhook"]).
    #[serde(default)]
    pub import_paths: Vec<String>,
    /// Maps short image name (e.g. "controller") to IMAGE_ env var name.
    pub images: HashMap<String, String>,
    /// Build system: "ko" (default) or "docker". If None, defaults to ko.
    #[serde(default)]
    pub build_system: Option<String>,
    /// Override prefix for InstallerSet matching. If None, uses component name.
    #[serde(default)]
    pub installer_set_prefix: Option<String>,
}

/// Top-level config: keys are component names, values are ComponentConfig.
#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct Config {
    pub components: HashMap<String, ComponentConfig>,
}

/// Load component configuration from a TOML file.
pub fn load_config(path: &Path) -> anyhow::Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let config: Config =
        toml::from_str(&content).with_context(|| format!("Failed to parse config: {}", path.display()))?;
    Ok(config)
}

/// Returns the default path to `config/components.toml` relative to the current directory.
pub fn default_config_path() -> PathBuf {
    PathBuf::from("config/components.toml")
}
