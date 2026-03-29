use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Idle,
    Reasoning,
    Text,
    Done,
}

#[allow(dead_code)]
pub struct StreamState {
    pub chat_id: String,
    pub thread_id: Option<i64>,
    pub is_dm: bool,
    pub msg_id: Option<i64>,
    pub stream_msg_id: Option<i64>,
    pub tool_msg_id: Option<i64>,
    pub tool_lines: Vec<String>,
    pub reasoning: String,
    pub text: String,
    pub phase: Phase,
    pub last_stream_update: Instant,
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
            tool_msg_id: None,
            tool_lines: Vec::new(),
            reasoning: String::new(),
            text: String::new(),
            phase: Phase::Idle,
            last_stream_update: Instant::now() - std::time::Duration::from_secs(10),
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
        let text = match self.phase {
            Phase::Reasoning => format!("💭 {}", self.reasoning),
            Phase::Text => self.text.clone(),
            _ => return None,
        };
        if text.is_empty() {
            return None;
        }
        // Truncate for Telegram message size limit
        if text.len() > 3900 {
            Some(format!("...{}", &text[text.len() - 3900..]))
        } else {
            Some(text)
        }
    }
}
