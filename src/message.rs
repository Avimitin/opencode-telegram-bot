use crate::access::{self, AccessCache, GateResult};
use crate::download::{download_telegram_file, AttachedFile};
use crate::markdown::sanitize_for_xml;
use crate::models::{build_model_keyboard, ModelCache};
use crate::opencode::{ModelRef, OpencodeClient, PromptPart};
use crate::session::BoundedMap;
use crate::stream::StreamState;
use crate::telegram::{CallbackQuery, Message, SendOpts, TelegramClient, Update};
use serde_json::json;
use std::collections::{HashMap, VecDeque};

pub struct QueuedMessage {
    pub chat_id: String,
    pub msg_id: i64,
    pub thread_id: Option<i64>,
    pub is_dm: bool,
    pub session_id: String,
    pub parts: Vec<PromptPart>,
    pub model: Option<ModelRef>,
}

pub struct BotState {
    pub tg: TelegramClient,
    pub oc: OpencodeClient,
    pub access_cache: AccessCache,
    pub model_cache: ModelCache,
    pub msg_sessions: BoundedMap<String>,
    pub msg_model_override: BoundedMap<String>,
    pub active_streams: HashMap<String, StreamState>,
    pub pending_queue: HashMap<String, VecDeque<QueuedMessage>>,
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
    let msg_id = msg.message_id;
    let thread_id = msg.message_thread_id;
    let reply_opts = SendOpts {
        reply_to_message_id: Some(msg_id),
        message_thread_id: thread_id,
        ..Default::default()
    };
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

    // /stat command — must reply to a bot message to identify the session
    if cmd_clean.starts_with("/stat") {
        let reply_to_bot = msg
            .reply_to_message
            .as_ref()
            .and_then(|r| r.from.as_ref())
            .map(|u| u.id == state.bot_id)
            .unwrap_or(false);

        if !reply_to_bot {
            let _ = state
                .tg
                .send_message(
                    &chat_id,
                    "Reply to a bot message with /stat to see session stats.",
                    &reply_opts,
                )
                .await;
            return;
        }

        // Try exact message match first, then fallback to most recent session in this chat
        let session_id = msg
            .reply_to_message
            .as_ref()
            .and_then(|r| {
                let key = format!("{}:{}", chat_id, r.message_id);
                state.msg_sessions.get(&key).cloned()
            })
            .or_else(|| {
                state
                    .msg_sessions
                    .find_last_by_prefix(&format!("{}:", chat_id))
                    .cloned()
            });

        if let Some(sid) = session_id {
            handle_stat(state, &chat_id, &sid, &reply_opts).await;
        } else {
            // No cached session — try fetching the latest from opencode
            match state.oc.session_list().await {
                Ok(sessions) => {
                    if let Some(s) = sessions.first() {
                        handle_stat(state, &chat_id, &s.id, &reply_opts).await;
                    } else {
                        let _ = state
                            .tg
                            .send_message(&chat_id, "No sessions found.", &reply_opts)
                            .await;
                    }
                }
                Err(_) => {
                    let _ = state
                        .tg
                        .send_message(
                            &chat_id,
                            "Could not find session for this message.",
                            &reply_opts,
                        )
                        .await;
                }
            }
        }
        return;
    }

    // /list_models command
    if cmd_clean.starts_with("/list_models") {
        let models = state.model_cache.get_models(&state.oc).await;
        if models.is_empty() {
            let _ = state
                .tg
                .send_message(&chat_id, "Failed to fetch model list.", &reply_opts)
                .await;
            return;
        }
        let lines: Vec<String> = std::iter::once("Available models:\n".to_string())
            .chain(models.iter().map(|m| format!("  {}", m.full_id)))
            .collect();
        let _ = state
            .tg
            .send_message(&chat_id, &lines.join("\n"), &reply_opts)
            .await;
        return;
    }

    // /model command (no args — show picker)
    if cmd_clean.trim() == "/model" {
        let models = state.model_cache.get_models(&state.oc).await;
        if models.is_empty() {
            let _ = state
                .tg
                .send_message(&chat_id, "Failed to fetch model list.", &reply_opts)
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
                    reply_to_message_id: Some(msg_id),
                    message_thread_id: thread_id,
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
                .send_message(&chat_id, &reply, &reply_opts)
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
                        &SendOpts {
                            reply_to_message_id: Some(msg_id),
                            message_thread_id: msg.message_thread_id,
                            ..Default::default()
                        },
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

    // When replying to another user's message (not the bot), include quoted context
    let mut quoted_context = String::new();
    if !reply_to_bot {
        if let Some(reply_msg) = &msg.reply_to_message {
            let reply_text = reply_msg
                .text
                .as_deref()
                .or(reply_msg.caption.as_deref())
                .unwrap_or("");
            if !reply_text.is_empty() {
                let reply_user = reply_msg
                    .from
                    .as_ref()
                    .and_then(|u| u.username.clone().or(Some(u.first_name.clone())))
                    .unwrap_or_else(|| "unknown".to_string());
                let reply_uid = reply_msg
                    .from
                    .as_ref()
                    .map(|u| u.id.to_string())
                    .unwrap_or_default();
                quoted_context = format!(
                    "<quoted-message user=\"{}\" user_id=\"{}\" message_id=\"{}\">\n{}\n</quoted-message>\n",
                    sanitize_for_xml(&reply_user),
                    reply_uid,
                    reply_msg.message_id,
                    sanitize_for_xml(reply_text)
                );
            }
        }
    }

    let prompt = format!(
        "<channel source=\"telegram\" chat_id=\"{}\" message_id=\"{}\" user=\"{}\" user_id=\"{}\" ts=\"{}\">\n{}{}\n</channel>",
        chat_id, msg_id, safe_username, sender_id, ts, quoted_context, safe_text
    );

    let thread_id = msg.message_thread_id;
    let is_dm = msg.chat.chat_type == "private";

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

    // If session is busy, queue the message instead of sending immediately
    if state.active_streams.contains_key(&session_id) {
        state
            .pending_queue
            .entry(session_id)
            .or_default()
            .push_back(QueuedMessage {
                chat_id: chat_id.to_string(),
                msg_id,
                thread_id,
                is_dm,
                session_id: String::new(), // filled by drain_queue
                parts,
                model,
            });
        return;
    }

    // Set up streaming
    dispatch_prompt(state, chat_id, msg_id, thread_id, is_dm, &session_id, parts, model).await;
}

/// Send a prompt and set up the streaming state. Used both for immediate
/// dispatch and for draining the pending queue.
pub async fn dispatch_prompt(
    state: &mut BotState,
    chat_id: &str,
    msg_id: i64,
    thread_id: Option<i64>,
    is_dm: bool,
    session_id: &str,
    parts: Vec<PromptPart>,
    model: Option<ModelRef>,
) {
    // Detect language from prompt text for the placeholder
    let has_cjk = parts.first()
        .and_then(|p| p.text.as_ref())
        .map(|t| t.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)))
        .unwrap_or(false);
    let placeholder = if has_cjk { "思考中…" } else { "Thinking…" };

    let mut placeholder_msg_id: Option<i64> = None;
    let opts = SendOpts {
        reply_to_message_id: Some(msg_id),
        message_thread_id: thread_id,
        reply_markup: Some(stop_button(session_id)),
        ..Default::default()
    };
    if let Ok(sent) = state.tg.send_message(chat_id, placeholder, &opts).await {
        placeholder_msg_id = Some(sent.message_id);
    }

    state.active_streams.insert(
        session_id.to_string(),
        StreamState::new(
            chat_id.to_string(),
            thread_id,
            is_dm,
            Some(msg_id),
            placeholder_msg_id,
        ),
    );

    // Fire prompt — don't block. The SSE handler in main.rs will drive
    // streaming updates and finalize the message on session.idle.
    let oc_base = state.oc.base_url.clone();
    let sid = session_id.to_string();
    tokio::spawn(async move {
        let client = OpencodeClient::new(&oc_base);
        if let Err(e) = client.session_prompt(&sid, parts, model).await {
            eprintln!("Prompt error: {}", e);
        }
    });
}

async fn handle_stat(state: &mut BotState, chat_id: &str, session_id: &str, opts: &SendOpts) {
    let messages = match state.oc.session_messages(session_id).await {
        Ok(m) => m,
        Err(e) => {
            let _ = state
                .tg
                .send_message(chat_id, &format!("Failed to fetch stats: {}", e), opts)
                .await;
            return;
        }
    };

    let mut total_input: i64 = 0;
    let mut total_output: i64 = 0;
    let mut total_reasoning: i64 = 0;
    let mut total_cache_read: i64 = 0;
    let mut model_id = String::new();
    let mut provider_id = String::new();
    let mut msg_count: usize = 0;

    for msg in &messages {
        let info = match msg.get("info") {
            Some(i) => i,
            None => continue,
        };
        let role = info.get("role").and_then(|v| v.as_str()).unwrap_or("");

        if role == "user" && model_id.is_empty() {
            if let Some(model) = info.get("model") {
                provider_id = model.get("providerID").and_then(|v| v.as_str()).unwrap_or("").to_string();
                model_id = model.get("modelID").and_then(|v| v.as_str()).unwrap_or("").to_string();
            }
        }

        if role == "assistant" {
            if let Some(tokens) = info.get("tokens") {
                total_input += tokens.get("input").and_then(|v| v.as_i64()).unwrap_or(0);
                total_output += tokens.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
                total_reasoning += tokens.get("reasoning").and_then(|v| v.as_i64()).unwrap_or(0);
                total_cache_read += tokens.get("cache")
                    .and_then(|c| c.get("read"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
            }
            msg_count += 1;
        }
    }

    let total_tokens = total_input + total_output + total_reasoning;
    let stat_text = format!(
        "Session: {}\n\
         Model: {}/{}\n\
         Turns: {}\n\
         \n\
         Tokens:\n\
         ├ Input: {}\n\
         ├ Output: {}\n\
         ├ Reasoning: {}\n\
         ├ Cache read: {}\n\
         └ Total: {}",
        &session_id[..session_id.len().min(20)],
        provider_id,
        model_id,
        msg_count,
        total_input,
        total_output,
        total_reasoning,
        total_cache_read,
        total_tokens,
    );

    let _ = state
        .tg
        .send_message(chat_id, &stat_text, opts)
        .await;
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
    } else if let Some(session_id) = data.strip_prefix("stop:") {
        // Abort the opencode session and finalize with current content
        let _ = state.oc.session_abort(session_id).await;
        // The SSE handler will receive session.idle and finalize the stream.
        // Answer the callback immediately.
        let _ = state
            .tg
            .answer_callback_query(&cb.id, Some("Stopping..."))
            .await;
    } else if data == "noop" {
        let _ = state.tg.answer_callback_query(&cb.id, None).await;
    }
}

pub fn stop_button(session_id: &str) -> serde_json::Value {
    json!({
        "inline_keyboard": [[{
            "text": "⏹ Stop",
            "callback_data": format!("stop:{}", session_id)
        }]]
    })
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
