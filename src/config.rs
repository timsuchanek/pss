use std::fs;
use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    pub openrouter_api_key: Option<String>,
    #[serde(default = "default_model")]
    pub model: String,

    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub sort: SortConfig,
    #[serde(default)]
    pub filters: FiltersConfig,
    #[serde(default)]
    pub sampling: SamplingConfig,
    #[serde(default)]
    pub state: StateConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UiConfig {
    pub sidebar_width: u16,
    pub chart_height: u16,
    pub recs_height: u16,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            sidebar_width: 54,
            chart_height: 18,
            recs_height: 8,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SortConfig {
    pub sidebar_key: String,   // "cpu" | "mem"
    pub sidebar_dir: String,   // "asc" | "desc"
    pub processes: String,     // "cpu" | "mem" | "name"
}

impl Default for SortConfig {
    fn default() -> Self {
        Self {
            sidebar_key: "cpu".into(),
            sidebar_dir: "desc".into(),
            processes: "cpu".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FiltersConfig {
    pub hide_kernel: bool,
    pub only_my_uid: bool,
    pub hide_self: bool,
}

impl Default for FiltersConfig {
    fn default() -> Self {
        Self {
            hide_kernel: true,
            only_my_uid: false,
            hide_self: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SamplingConfig {
    pub interval_ms: u64,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self { interval_ms: 1000 }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StateConfig {
    #[serde(default)]
    pub collapsed: Vec<String>,
}

fn default_model() -> String {
    "anthropic/claude-haiku-4.5".into()
}

pub fn load() -> Config {
    let mut cfg = file_config().unwrap_or_default();
    if cfg.model.is_empty() {
        cfg.model = default_model();
    }
    if cfg.openrouter_api_key.is_none() {
        cfg.openrouter_api_key = std::env::var("OPENROUTER_API_KEY").ok();
    }
    cfg
}

fn file_config() -> Option<Config> {
    let path = config_path()?;
    let text = fs::read_to_string(&path).ok()?;
    toml::from_str(&text).ok()
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let Some(path) = config_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Don't write the api key back if it originated from the env — only
    // persist it if the file already had one.
    let existing_key = fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str::<Config>(&s).ok())
        .and_then(|c| c.openrouter_api_key);
    let mut write_cfg = cfg.clone();
    if existing_key.is_none() {
        write_cfg.openrouter_api_key = None;
    }
    let serialized = toml::to_string_pretty(&write_cfg).unwrap_or_default();
    fs::write(path, serialized)
}

pub fn config_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "pss")?;
    Some(dirs.config_dir().join("config.toml"))
}
