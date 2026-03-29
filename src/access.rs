use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    pub dm_policy: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub groups: HashMap<String, GroupPolicy>,
    #[serde(default)]
    pub pending: HashMap<String, PendingEntry>,
    #[serde(default)]
    pub mention_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPolicy {
    pub require_mention: bool,
    #[serde(default)]
    pub allow_from: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingEntry {
    pub sender_id: String,
    pub chat_id: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub replies: u32,
}

impl Default for Access {
    fn default() -> Self {
        Access {
            dm_policy: "pairing".to_string(),
            allow_from: vec![],
            groups: HashMap::new(),
            pending: HashMap::new(),
            mention_patterns: vec![],
        }
    }
}

pub struct AccessCache {
    data: Option<Access>,
    loaded_at: Option<Instant>,
    path: std::path::PathBuf,
}

const CACHE_TTL_SECS: f64 = 2.0;

impl AccessCache {
    pub fn new(path: std::path::PathBuf) -> Self {
        AccessCache {
            data: None,
            loaded_at: None,
            path,
        }
    }

    pub fn load(&mut self) -> Access {
        if let (Some(data), Some(loaded_at)) = (&self.data, &self.loaded_at)
            && loaded_at.elapsed().as_secs_f64() < CACHE_TTL_SECS {
                return data.clone();
            }
        let access = match fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Access::default(),
        };
        self.data = Some(access.clone());
        self.loaded_at = Some(Instant::now());
        access
    }

    pub fn save(&mut self, access: &Access) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(access) {
            let _ = fs::write(&self.path, json);
        }
        self.data = Some(access.clone());
        self.loaded_at = Some(Instant::now());
    }
}

#[derive(Debug, PartialEq)]
pub enum GateResult {
    Allow,
    Pair,
    Deny,
}

pub fn get_mention_patterns(access: &Access, bot_username: &str) -> Vec<String> {
    let mut patterns = access.mention_patterns.clone();
    if !bot_username.is_empty() {
        let at_username = format!("@{}", bot_username);
        if !patterns.contains(&at_username) {
            patterns.push(at_username);
        }
    }
    patterns
}

pub struct GateContext<'a> {
    pub access: &'a Access,
    pub chat_type: &'a str,
    pub chat_id: &'a str,
    pub sender_id: &'a str,
    pub text: &'a str,
    pub bot_username: &'a str,
    pub reply_to_bot: bool,
}

pub fn gate(ctx: &GateContext) -> GateResult {
    let access = ctx.access;
    let chat_type = ctx.chat_type;
    let chat_id = ctx.chat_id;
    let sender_id = ctx.sender_id;
    let text = ctx.text;
    let bot_username = ctx.bot_username;
    let reply_to_bot = ctx.reply_to_bot;
    // Group message
    if chat_type == "group" || chat_type == "supergroup" {
        let group_policy = match access.groups.get(chat_id) {
            Some(p) => p,
            None => return GateResult::Deny,
        };

        if group_policy.require_mention {
            let patterns = get_mention_patterns(access, bot_username);
            let trimmed = text.trim_start();
            let starts_with_mention = patterns.iter().any(|p| trimmed.starts_with(p));
            if !starts_with_mention && !reply_to_bot {
                return GateResult::Deny;
            }
        }

        if !group_policy.allow_from.is_empty()
            && !group_policy.allow_from.contains(&sender_id.to_string())
        {
            return GateResult::Deny;
        }
        return GateResult::Allow;
    }

    // DM
    if access.dm_policy == "disabled" {
        return GateResult::Deny;
    }
    if access.allow_from.contains(&sender_id.to_string()) {
        return GateResult::Allow;
    }
    if access.dm_policy == "pairing" {
        return GateResult::Pair;
    }
    GateResult::Deny
}

pub fn strip_mention(text: &str, access: &Access, bot_username: &str) -> String {
    let patterns = get_mention_patterns(access, bot_username);
    let trimmed = text.trim_start();
    for p in &patterns {
        if trimmed.starts_with(p) {
            return trimmed[p.len()..].trim_start().to_string();
        }
    }
    text.to_string()
}

pub fn handle_pairing(access: &mut Access, sender_id: &str, chat_id: &str) -> String {
    if access.allow_from.contains(&sender_id.to_string()) {
        return "You are already paired. Send messages and I will respond.".to_string();
    }

    // Check existing pending
    let existing = access
        .pending
        .iter_mut()
        .find(|(_, v)| v.sender_id == sender_id);

    if let Some((code, entry)) = existing {
        let code = code.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        if entry.expires_at > now {
            entry.replies += 1;
            if entry.replies > 3 {
                access.pending.remove(&code);
                return "Too many attempts. Please try again later.".to_string();
            }
            return format!(
                "Your pairing code is: {}\nAsk the admin to run: opencode telegram pair {}",
                code, code
            );
        }
        access.pending.remove(&code);
    }

    // Clean expired
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    access.pending.retain(|_, v| v.expires_at > now);

    if access.pending.len() >= 3 {
        return "Too many pending pairing requests. Please try again later.".to_string();
    }

    let code = generate_pairing_code();
    access.pending.insert(
        code.clone(),
        PendingEntry {
            sender_id: sender_id.to_string(),
            chat_id: chat_id.to_string(),
            created_at: now,
            expires_at: now + 3_600_000,
            replies: 1,
        },
    );

    format!(
        "Your pairing code is: {}\nAsk the admin to approve with: opencode telegram pair {}\nThis code expires in 1 hour.",
        code, code
    )
}

fn generate_pairing_code() -> String {
    let bytes: [u8; 3] = rand::random();
    hex::encode(&bytes)
}

// Inline hex encoding to avoid extra dependency
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}
