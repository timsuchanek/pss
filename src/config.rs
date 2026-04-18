use std::fs;

use directories::ProjectDirs;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    pub openrouter_api_key: Option<String>,
    #[serde(default = "default_model")]
    pub model: String,
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
    let dirs = ProjectDirs::from("", "", "pss")?;
    let path = dirs.config_dir().join("config.toml");
    let text = fs::read_to_string(&path).ok()?;
    toml_lite::from_str(&text).ok()
}

mod toml_lite {
    use super::Config;
    pub fn from_str(s: &str) -> Result<Config, ()> {
        let mut cfg = Config::default();
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once('=').ok_or(())?;
            let k = k.trim();
            let v = v.trim().trim_matches('"').to_string();
            match k {
                "openrouter_api_key" => cfg.openrouter_api_key = Some(v),
                "model" => cfg.model = v,
                _ => {}
            }
        }
        Ok(cfg)
    }
}
