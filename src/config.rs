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
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let home_dir = PathBuf::from(env::var("HOME").unwrap_or_else(|_| {
            dirs_home().unwrap_or_else(|| "/tmp".to_string())
        }));

        let state_dir = PathBuf::from(
            env::var("TELEGRAM_STATE_DIR").unwrap_or_else(|_| {
                home_dir
                    .join(".opencode")
                    .join("channels")
                    .join("telegram")
                    .to_string_lossy()
                    .to_string()
            }),
        );

        let access_file = state_dir.join("access.json");
        let approved_dir = state_dir.join("approved");

        let bot_token = env::var("TELEGRAM_BOT_TOKEN").map_err(|_| {
            "TELEGRAM_BOT_TOKEN required (set via environment or systemd EnvironmentFile)".to_string()
        })?;

        // Load opencode.json
        let opencode_config_path = home_dir.join("opencode.json");
        let opencode_config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&opencode_config_path).map_err(|e| {
                format!("Failed to read {}: {}", opencode_config_path.display(), e)
            })?)?;

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

fn dirs_home() -> Option<String> {
    env::var("HOME").ok()
}
