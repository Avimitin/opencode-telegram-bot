use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::PathBuf;

#[allow(dead_code)]
pub struct Config {
    pub bot_token: String,
    pub state_dir: PathBuf,
    pub access_file: PathBuf,
    pub approved_dir: PathBuf,
    pub opencode_config: serde_json::Value,
    pub home_dir: PathBuf,
}

impl Config {
    pub fn load() -> Result<Self> {
        let home_dir = PathBuf::from(env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()));

        let xdg_config = env::var("XDG_CONFIG_HOME")
            .unwrap_or_else(|_| home_dir.join(".config").to_string_lossy().to_string());
        let state_dir = PathBuf::from(
            env::var("TELEGRAM_STATE_DIR")
                .unwrap_or_else(|_| format!("{}/opencode_telegram_bot", xdg_config)),
        );

        let access_file = state_dir.join("access.json");
        let approved_dir = state_dir.join("approved");

        let bot_token = env::var("TELEGRAM_BOT_TOKEN")
            .context("TELEGRAM_BOT_TOKEN required (set via environment or systemd EnvironmentFile)")?;

        let opencode_config_path = PathBuf::from(
            env::var("OPENCODE_CONFIG_PATH")
                .unwrap_or_else(|_| format!("{}/opencode/opencode.json", xdg_config)),
        );
        let opencode_config: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&opencode_config_path)
                .with_context(|| format!("Failed to read {}", opencode_config_path.display()))?,
        )?;

        Ok(Config {
            bot_token,
            state_dir,
            access_file,
            approved_dir,
            opencode_config,
            home_dir,
        })
    }
}
