use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Idle,
    Reasoning,
    Text,
}

#[allow(dead_code)]
pub struct StreamState {
    pub chat_id: String,
    pub thread_id: Option<i64>,
    pub is_dm: bool,
    pub msg_id: Option<i64>,
    pub stream_msg_id: Option<i64>,
    pub tool_lines: Vec<String>,
    pub reasoning: String,
    pub text: String,
    pub error: Option<String>,
    pub phase: Phase,
    pub last_stream_update: Instant,
    pub last_activity: Instant,
    pub created_at: Instant,
}

// Telegram rate limit ~20 edits/min per chat
pub const EDIT_THROTTLE_MS: u64 = 1500;
pub const DM_THROTTLE_MS: u64 = 300;

impl StreamState {
    pub fn new(
        chat_id: String,
        thread_id: Option<i64>,
        is_dm: bool,
        msg_id: Option<i64>,
        stream_msg_id: Option<i64>,
    ) -> Self {
        StreamState {
            chat_id,
            thread_id,
            is_dm,
            msg_id,
            stream_msg_id,
            tool_lines: Vec::new(),
            reasoning: String::new(),
            text: String::new(),
            error: None,
            phase: Phase::Idle,
            last_stream_update: Instant::now() - std::time::Duration::from_secs(10),
            last_activity: Instant::now(),
            created_at: Instant::now(),
        }
    }

    pub fn should_update(&self) -> bool {
        let throttle = if self.is_dm {
            DM_THROTTLE_MS
        } else {
            EDIT_THROTTLE_MS
        };
        self.last_stream_update.elapsed().as_millis() >= throttle as u128
    }

    pub fn mark_updated(&mut self) {
        self.last_stream_update = Instant::now();
    }

    pub fn display_text(&self) -> Option<String> {
        let mut parts = Vec::new();

        // Tool calls section
        if !self.tool_lines.is_empty() {
            parts.push(self.tool_lines.join("\n"));
        }

        // Current phase content
        match self.phase {
            Phase::Reasoning => {
                if !self.reasoning.is_empty() {
                    parts.push(format!("💭 {}", self.reasoning));
                }
            }
            Phase::Text => {
                if !self.text.is_empty() {
                    parts.push(self.text.clone());
                }
            }
            Phase::Idle => {
                // Tool-only updates (no reasoning/text yet)
                if parts.is_empty() {
                    return None;
                }
            }
        }

        if parts.is_empty() {
            return None;
        }

        let text = parts.join("\n\n");
        // Truncate for Telegram message size limit
        if text.len() > 3900 {
            Some(format!("...{}", &text[text.len() - 3900..]))
        } else {
            Some(text)
        }
    }
}
