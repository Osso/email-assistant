use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    pub provider: Option<String>,
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("email-assistant")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn profile_path() -> PathBuf {
    config_dir().join("profile.md")
}

pub fn predictions_path() -> PathBuf {
    config_dir().join("predictions.json")
}

pub fn labels_path() -> PathBuf {
    config_dir().join("labels.json")
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = config_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(config_path(), content)?;
        Ok(())
    }

    pub fn default_provider(&self) -> &str {
        self.provider.as_deref().unwrap_or("gmail")
    }
}
