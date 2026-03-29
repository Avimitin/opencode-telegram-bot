use crate::access::{self, AccessCache, GateResult};
use crate::download::{download_telegram_file, AttachedFile};
use crate::markdown::sanitize_for_xml;
use crate::models::{build_model_keyboard, ModelCache};
use crate::opencode::{ModelRef, OpencodeClient, PromptPart};
use crate::session::BoundedMap;
use crate::stream::StreamState;
use crate::telegram::{CallbackQuery, Message, SendOpts, TelegramClient, Update};
use serde_json::json;
use std::collections::HashMap;

pub struct BotState {
    pub tg: TelegramClient,
    pub oc: OpencodeClient,
    pub access_cache: AccessCache,
    pub model_cache: ModelCache,
    pub msg_sessions: BoundedMap<String>,
    pub msg_model_override: BoundedMap<String>,
    pub active_streams: HashMap<String, StreamState>,
    pub bot_id: i64,
    pub bot_username: String,
}

pub async fn handle_update(state: &mut BotState, update: &Update) {
    if let Some(ref msg) = update.message {
        let text;
        let mut files: Vec<AttachedFile> = Vec::new();

        if let Some(ref photos) = msg.photo {
            // Get largest photo
            if let Some(largest) = photos.last() {
                if let Some(file) = download_telegram_file(
                    &state.tg,
                    &largest.file_id,
                    "image/jpeg",
                    "photo.jpg",
                )
                .await
                {
                    files.push(file);
                }
            }
            text = msg.caption.clone().unwrap_or_else(|| "(photo)".to_string());
        } else if let Some(ref doc) = msg.document {
            let mime = doc
                .mime_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let filename = doc
                .file_name
                .clone()
                .unwrap_or_else(|| "file".to_string());
            if mime.starts_with("image/") {
                if let Some(file) =
                    download_telegram_file(&state.tg, &doc.file_id, &mime, &filename).await
                {
                    files.push(file);
                }
            }
            text = msg
                .caption
                .clone()
                .unwrap_or_else(|| format!("(document: {})", filename));
        } else if msg.voice.is_some() {
            text = msg
                .caption
                .clone()
                .unwrap_or_else(|| "(voice message)".to_string());
        } else if msg.video.is_some() {
            text = msg
                .caption
                .clone()
                .unwrap_or_else(|| "(video)".to_string());
        } else if let Some(ref sticker) = msg.sticker {
            let emoji = sticker
                .emoji
                .as_ref()
                .map(|e| format!(" {}", e))
                .unwrap_or_default();
            text = format!("(sticker{})", emoji);
        } else if let Some(ref t) = msg.text {
            text = t.clone();
        } else {
            return;
        }

        handle_message(state, msg, &text, files).await;
    }

    if let Some(ref cb) = update.callback_query {
        handle_callback(state, cb).await;
    }
}

async fn handle_message(
    state: &mut BotState,
    msg: &Message,
    text: &str,
    files: Vec<AttachedFile>,
) {
    let chat_id = msg.chat.id.to_string();
    let sender_id = msg.from.as_ref().map(|u| u.id.to_string()).unwrap_or_default();
    let access = state.access_cache.load();

    // Strip mention and parse commands
    let cmd = access::strip_mention(text, &access, &state.bot_username)
        .trim_start()
        .to_string();
    // Also strip @botname from commands like /list_models@botname
    let cmd_clean: String = if let Some(pos) = cmd.find('@') {
        let end = cmd[pos..].find(' ').map(|p| pos + p).unwrap_or(cmd.len());
        format!("{}{}", &cmd[..pos], &cmd[end..])
    } else {
        cmd.clone()
    };
    let cmd_clean = cmd_clean.trim_start();

    // /list_models command
    if cmd_clean.starts_with("/list_models") {
        let models = state.model_cache.get_models(&state.oc).await;
        if models.is_empty() {
            let _ = state
                .tg
                .send_message(&chat_id, "Failed to fetch model list.", &SendOpts::default())
                .await;
            return;
        }
        let lines: Vec<String> = std::iter::once("Available models:\n".to_string())
            .chain(models.iter().map(|m| format!("  {}", m.full_id)))
            .collect();
        let _ = state
            .tg
            .send_message(&chat_id, &lines.join("\n"), &SendOpts::default())
            .await;
        return;
    }

    // /model command (no args — show picker)
    if cmd_clean.trim() == "/model" {
        let models = state.model_cache.get_models(&state.oc).await;
        if models.is_empty() {
            let _ = state
                .tg
                .send_message(&chat_id, "Failed to fetch model list.", &SendOpts::default())
                .await;
            return;
        }
        let markup = build_model_keyboard(&models, 0);
        let _ = state
            .tg
            .send_message(
                &chat_id,
                "Select a model:",
                &SendOpts {
                    reply_markup: Some(markup),
                    ..Default::default()
                },
            )
            .await;
        return;
    }

    // Gate check
    let reply_to_bot = msg
        .reply_to_message
        .as_ref()
        .and_then(|r| r.from.as_ref())
        .map(|u| u.id == state.bot_id)
        .unwrap_or(false);

    let gate_result = access::gate(&access::GateContext {
        access: &access,
        chat_type: &msg.chat.chat_type,
        chat_id: &chat_id,
        sender_id: &sender_id,
        text,
        bot_username: &state.bot_username,
        reply_to_bot,
    });

    match gate_result {
        GateResult::Deny => return,
        GateResult::Pair => {
            let mut access = state.access_cache.load();
            let reply = access::handle_pairing(&mut access, &sender_id, &chat_id);
            state.access_cache.save(&access);
            let _ = state
                .tg
                .send_message(&chat_id, &reply, &SendOpts::default())
                .await;
            return;
        }
        GateResult::Allow => {}
    }

    process_message(state, msg, text, &chat_id, &sender_id, files).await;
}

async fn process_message(
    state: &mut BotState,
    msg: &Message,
    text: &str,
    chat_id: &str,
    sender_id: &str,
    files: Vec<AttachedFile>,
) {
    let msg_id = msg.message_id;
    let access = state.access_cache.load();
    let mut clean_text = access::strip_mention(text, &access, &state.bot_username);

    // Parse /model override
    let mut model_override: Option<String> = None;
    if let Some(caps) = regex::Regex::new(r"^/model[= ](\S+)\s*(.*)$")
        .ok()
        .and_then(|re| re.captures(&clean_text))
    {
        model_override = Some(caps.get(1).unwrap().as_str().to_string());
        clean_text = caps.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();

        // Model-only message (no prompt)
        if clean_text.is_empty() && files.is_empty() {
            if let Ok(sent) = state
                .tg
                .send_message(
                    chat_id,
                    &format!(
                        "Model set to {}. Reply to this message to start a new session.",
                        model_override.as_ref().unwrap()
                    ),
                    &SendOpts::default(),
                )
                .await
            {
                state.msg_model_override.insert(
                    format!("{}:{}", chat_id, sent.message_id),
                    model_override.unwrap(),
                );
            }
            return;
        }
    }

    // Check reply to model-set confirmation
    let reply_to_bot = msg
        .reply_to_message
        .as_ref()
        .and_then(|r| r.from.as_ref())
        .map(|u| u.id == state.bot_id)
        .unwrap_or(false);

    if model_override.is_none() && reply_to_bot {
        if let Some(reply_msg) = &msg.reply_to_message {
            let key = format!("{}:{}", chat_id, reply_msg.message_id);
            model_override = state.msg_model_override.remove(&key);
        }
    }

    // Detect @mention → force new session in groups
    let is_group = msg.chat.chat_type == "group" || msg.chat.chat_type == "supergroup";
    let patterns = access::get_mention_patterns(&access, &state.bot_username);
    let starts_with_mention = patterns.iter().any(|p| text.trim_start().starts_with(p));
    let force_new_session = is_group && starts_with_mention;

    let prompt_text = if clean_text.is_empty() {
        text.to_string()
    } else {
        clean_text
    };

    // React to acknowledge
    let _ = state
        .tg
        .set_message_reaction(
            chat_id,
            msg_id,
            &[json!({"type": "emoji", "emoji": "👀"})],
        )
        .await;

    // Resolve session
    let mut session_id: Option<String> = None;

    if !force_new_session {
        if reply_to_bot {
            if let Some(reply_msg) = &msg.reply_to_message {
                let key = format!("{}:{}", chat_id, reply_msg.message_id);
                session_id = state.msg_sessions.get(&key).cloned();
            }
        }
        if session_id.is_none() {
            session_id = state
                .msg_sessions
                .find_last_by_prefix(&format!("{}:", chat_id))
                .cloned();
        }
    }

    if session_id.is_none() {
        let username = msg
            .from
            .as_ref()
            .and_then(|u| u.username.clone().or(Some(u.first_name.clone())))
            .unwrap_or_else(|| "unknown".to_string());
        let chat_title = if msg.chat.chat_type == "private" {
            format!("Telegram DM: {}", username)
        } else {
            format!(
                "Telegram: {}",
                msg.chat.title.as_deref().unwrap_or(chat_id)
            )
        };

        match state.oc.session_create(&chat_title).await {
            Ok(session) => session_id = Some(session.id),
            Err(e) => {
                eprintln!("Session create error: {}", e);
                let _ = state
                    .tg
                    .send_message(
                        chat_id,
                        "Failed to create session. Please try again.",
                        &SendOpts::default(),
                    )
                    .await;
                return;
            }
        }
    }

    let session_id = session_id.unwrap();

    // Build prompt
    let username = msg
        .from
        .as_ref()
        .and_then(|u| u.username.clone().or(Some(u.first_name.clone())))
        .unwrap_or_else(|| "unknown".to_string());
    let ts = chrono_now_iso();
    let safe_text = sanitize_for_xml(&prompt_text);
    let safe_username = sanitize_for_xml(&username);
    let prompt = format!(
        "<channel source=\"telegram\" chat_id=\"{}\" message_id=\"{}\" user=\"{}\" user_id=\"{}\" ts=\"{}\">\n{}\n</channel>",
        chat_id, msg_id, safe_username, sender_id, ts, safe_text
    );

    // Set up streaming
    let thread_id = msg.message_thread_id;
    let is_dm = msg.chat.chat_type == "private";

    let mut placeholder_msg_id: Option<i64> = None;
    if !is_dm {
        let mut opts = SendOpts {
            reply_to_message_id: Some(msg_id),
            ..Default::default()
        };
        if let Some(tid) = thread_id {
            opts.message_thread_id = Some(tid);
        }
        if let Ok(sent) = state.tg.send_message(chat_id, "⏳", &opts).await {
            placeholder_msg_id = Some(sent.message_id);
        }
    }

    state.active_streams.insert(
        session_id.clone(),
        StreamState::new(
            chat_id.to_string(),
            thread_id,
            is_dm,
            Some(msg_id),
            placeholder_msg_id,
        ),
    );

    // Build prompt parts
    let mut parts = vec![PromptPart {
        part_type: "text".to_string(),
        text: Some(prompt),
        mime: None,
        url: None,
        filename: None,
    }];
    for file in &files {
        parts.push(PromptPart {
            part_type: "file".to_string(),
            text: None,
            mime: Some(file.mime.clone()),
            url: Some(file.data_url.clone()),
            filename: Some(file.filename.clone()),
        });
    }

    // Parse model override
    let model = model_override.and_then(|m| {
        let slash = m.find('/')?;
        Some(ModelRef {
            provider_id: m[..slash].to_string(),
            model_id: m[slash + 1..].to_string(),
        })
    });

    // Fire prompt — don't block. The SSE handler in main.rs will drive
    // streaming updates and finalize the message on session.idle.
    let oc_base = state.oc.base_url.clone();
    let sid = session_id.clone();
    tokio::spawn(async move {
        let client = OpencodeClient::new(&oc_base);
        if let Err(e) = client.session_prompt(&sid, parts, model).await {
            eprintln!("Prompt error: {}", e);
        }
    });
}

async fn handle_callback(state: &mut BotState, cb: &CallbackQuery) {
    let data = match &cb.data {
        Some(d) => d.as_str(),
        None => return,
    };
    let chat_id = cb
        .message
        .as_ref()
        .map(|m| m.chat.id.to_string())
        .unwrap_or_default();

    if let Some(model) = data.strip_prefix("model:") {
        let _ = state
            .tg
            .edit_message_text(
                &chat_id,
                cb.message.as_ref().map(|m| m.message_id).unwrap_or(0),
                &format!(
                    "Model set to {}. Reply to this message to start a new session.",
                    model
                ),
            )
            .await;
        if let Some(msg_id) = cb.message.as_ref().map(|m| m.message_id) {
            state
                .msg_model_override
                .insert(format!("{}:{}", chat_id, msg_id), model.to_string());
        }
        let _ = state
            .tg
            .answer_callback_query(&cb.id, Some(&format!("Selected: {}", model)))
            .await;
    } else if let Some(page_str) = data.strip_prefix("modelpage:") {
        if let Ok(page) = page_str.parse::<usize>() {
            let models = state.model_cache.get_models(&state.oc).await;
            let markup = build_model_keyboard(&models, page);
            let _ = state
                .tg
                .edit_message_reply_markup(
                    &chat_id,
                    cb.message.as_ref().map(|m| m.message_id).unwrap_or(0),
                    &markup,
                )
                .await;
            let _ = state.tg.answer_callback_query(&cb.id, None).await;
        }
    } else if data == "noop" {
        let _ = state.tg.answer_callback_query(&cb.id, None).await;
    }
}

fn chrono_now_iso() -> String {
    // Simple ISO 8601 timestamp without chrono dependency
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Good enough for a timestamp
    format!("{}Z", secs)
}
