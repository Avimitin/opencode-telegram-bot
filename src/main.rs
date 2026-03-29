mod access;
mod config;
mod download;
mod markdown;
mod message;
mod models;
mod opencode;
mod session;
mod sse;
mod stream;
mod telegram;

use crate::access::AccessCache;
use crate::config::Config;
use crate::markdown::{split_message, thinking_to_md2, to_markdown_v2, tools_to_md2};
use crate::message::BotState;
use crate::models::ModelCache;
use crate::opencode::{OpencodeClient, OpencodeServer};
use crate::session::BoundedMap;
use crate::sse::{SseEvent, SseStream};
use crate::stream::{Phase, StreamState};
use crate::telegram::{SendOpts, TelegramClient};
use serde_json::json;
use std::collections::HashMap;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Load config
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Start opencode server
    println!("Starting opencode server...");
    let server = match OpencodeServer::spawn(&config.opencode_config).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to start opencode: {}", e);
            std::process::exit(1);
        }
    };
    println!("Opencode server ready: {}", server.url);

    let oc = OpencodeClient::new(&server.url);

    // Create Telegram bot
    let tg = TelegramClient::new(&config.bot_token);

    let me = match tg.get_me().await {
        Ok(u) => u,
        Err(e) => {
            eprintln!("Failed to get bot info: {}", e);
            std::process::exit(1);
        }
    };
    let bot_username = me.username.clone().unwrap_or_default();
    println!("Bot username: @{}", bot_username);

    // Set commands
    let _ = tg
        .set_my_commands(&[
            json!({"command": "list_models", "description": "List available models"}),
            json!({"command": "model", "description": "Set model: /model provider/model"}),
            json!({"command": "stat", "description": "Session stats (reply to a bot message)"}),
        ])
        .await;

    // Subscribe to SSE events
    let mut sse_stream = connect_sse(&oc).await;
    println!("SSE subscriber connected");

    // Build bot state
    let mut state = BotState {
        tg,
        oc,
        access_cache: AccessCache::new(config.access_file.clone()),
        model_cache: ModelCache::new(),
        msg_sessions: BoundedMap::new(5000),
        msg_model_override: BoundedMap::new(5000),
        active_streams: HashMap::new(),
        pending_queue: HashMap::new(),
        bot_id: me.id,
        bot_username,
    };

    println!(
        "Telegram bot @{} is running!",
        me.username.as_deref().unwrap_or("?")
    );

    // Main loop: concurrently handle Telegram updates and SSE events via select!.
    // This eliminates the deadlock where handle_update blocked the main loop,
    // preventing SSE events (including session.idle) from ever being processed.
    let mut update_offset: i64 = 0;
    let mut approved_interval = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut stale_interval = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        tokio::select! {
            biased;

            // SSE events — high priority to keep streaming responsive
            event = sse_stream.next_event() => {
                match event {
                    Some(event) => handle_sse_event(&mut state, event).await,
                    None => {
                        eprintln!("SSE disconnected, reconnecting...");
                        sse_stream = reconnect_sse(&state.oc).await;
                    }
                }
            }

            // Telegram updates
            result = state.tg.get_updates(update_offset, 5) => {
                match result {
                    Ok(updates) => {
                        for update in &updates {
                            update_offset = update.update_id + 1;
                            message::handle_update(&mut state, update).await;
                        }
                    }
                    Err(e) => {
                        eprintln!("getUpdates error: {}", e);
                    }
                }
            }

            // Poll approved pairing directory
            _ = approved_interval.tick() => {
                poll_approved(&state.tg, &config.approved_dir).await;
            }

            // Clean up timed-out streams (failsafe for lost session.idle events)
            _ = stale_interval.tick() => {
                let stale: Vec<String> = state
                    .active_streams
                    .iter()
                    .filter(|(_, s)| s.last_activity.elapsed().as_secs() > 300)
                    .map(|(k, _)| k.clone())
                    .collect();
                for id in stale {
                    if let Some(stream) = state.active_streams.remove(&id) {
                        eprintln!("Stream {} timed out", id);
                        finalize_stream(&mut state, &id, stream).await;
                    }
                }
            }
        }
    }
}

async fn connect_sse(oc: &OpencodeClient) -> SseStream {
    match oc.event_subscribe().await {
        Ok(r) => SseStream::new(r),
        Err(e) => {
            eprintln!("Failed to subscribe to SSE: {}", e);
            std::process::exit(1);
        }
    }
}

/// Reconnect to SSE with exponential backoff (capped at 30s).
async fn reconnect_sse(oc: &OpencodeClient) -> SseStream {
    let mut delay = std::time::Duration::from_secs(2);
    loop {
        tokio::time::sleep(delay).await;
        match oc.event_subscribe().await {
            Ok(r) => {
                eprintln!("SSE reconnected");
                return SseStream::new(r);
            }
            Err(e) => {
                eprintln!("SSE reconnect failed: {}", e);
                delay = (delay * 2).min(std::time::Duration::from_secs(30));
            }
        }
    }
}

async fn handle_sse_event(state: &mut BotState, event: SseEvent) {
    // opencode puts the event type in data.type, not in the SSE event: header
    let event_type = event
        .data
        .get("type")
        .and_then(|v| v.as_str())
        .or_else(|| {
            if !event.event_type.is_empty() {
                Some(event.event_type.as_str())
            } else {
                None
            }
        })
        .unwrap_or("");

    if event_type == "message.part.updated" {
        let props = &event.data;
        let session_id = props
            .get("properties")
            .and_then(|p| p.get("sessionID"))
            .or_else(|| {
                props
                    .get("properties")
                    .and_then(|p| p.get("part"))
                    .and_then(|p| p.get("sessionID"))
            })
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let part = props
            .get("properties")
            .and_then(|p| p.get("part"))
            .cloned()
            .unwrap_or_default();

        let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if let Some(stream) = state.active_streams.get_mut(&session_id) {
            stream.last_activity = std::time::Instant::now();
            match part_type {
                "reasoning" => {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        stream.phase = Phase::Reasoning;
                        stream.reasoning = text.to_string();
                    }
                }
                "text" => {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        stream.phase = Phase::Text;
                        stream.text = text.to_string();
                    }
                }
                "tool" => {
                    let status = part
                        .get("state")
                        .and_then(|s| s.get("status"))
                        .and_then(|v| v.as_str());
                    if status == Some("completed") {
                        let tool_name =
                            part.get("tool").and_then(|v| v.as_str()).unwrap_or("?");
                        let title = part
                            .get("state")
                            .and_then(|s| s.get("title"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("done");
                        stream
                            .tool_lines
                            .push(format!("🔧 {} — {}", tool_name, title));

                        let tool_text = stream.tool_lines.join("\n");
                        let chat_id = stream.chat_id.clone();

                        if let Some(tool_msg_id) = stream.tool_msg_id {
                            let _ = state
                                .tg
                                .edit_message_text(&chat_id, tool_msg_id, &tool_text)
                                .await;
                        } else {
                            let mut opts = SendOpts::default();
                            if let Some(tid) = stream.thread_id {
                                opts.message_thread_id = Some(tid);
                            }
                            if let Ok(sent) =
                                state.tg.send_message(&chat_id, &tool_text, &opts).await
                            {
                                stream.tool_msg_id = Some(sent.message_id);
                            }
                        }
                    }
                }
                _ => {}
            }

            // Throttled streaming display
            if (part_type == "reasoning" || part_type == "text") && stream.should_update() {
                if let Some(display) = stream.display_text() {
                    let chat_id = stream.chat_id.clone();
                    if let Some(stream_msg_id) = stream.stream_msg_id {
                        let markup = message::stop_button(&session_id);
                        let _ = state
                            .tg
                            .edit_message_text_markup(&chat_id, stream_msg_id, &display, &markup)
                            .await;
                    }
                    stream.mark_updated();
                }
            }
        }
    }

    if event_type == "session.idle" {
        let session_id = event
            .data
            .get("properties")
            .and_then(|p| p.get("sessionID"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(stream) = state.active_streams.remove(&session_id) {
            finalize_stream(state, &session_id, stream).await;
        }
    }
}

/// Send the final response message and clean up after a stream completes.
async fn finalize_stream(state: &mut BotState, session_id: &str, stream: StreamState) {
    let chat_id = &stream.chat_id;

    // Delete streaming placeholder and tool message
    if let Some(mid) = stream.stream_msg_id {
        let _ = state.tg.delete_message(chat_id, mid).await;
    }
    if let Some(mid) = stream.tool_msg_id {
        let _ = state.tg.delete_message(chat_id, mid).await;
    }

    // Build final message: tool calls + reasoning + response text
    let mut final_text = String::new();
    if !stream.tool_lines.is_empty() {
        final_text.push_str(&tools_to_md2(&stream.tool_lines.join("\n")));
        final_text.push_str("\n\n");
    }
    if !stream.reasoning.is_empty() {
        final_text.push_str(&thinking_to_md2(&stream.reasoning));
        final_text.push_str("\n\n");
    }
    let response_text = if stream.text.is_empty() {
        "(no response)".to_string()
    } else {
        // Strip <channel>...</channel> tags the LLM may echo back
        let re = regex::Regex::new(r"(?s)<channel\b[^>]*>.*?</channel>\n?").unwrap();
        re.replace_all(&stream.text, "").trim().to_string()
    };
    if response_text.is_empty() {
        return;
    }
    final_text.push_str(&to_markdown_v2(&response_text));

    // Send final message
    let send_opts = SendOpts {
        reply_to_message_id: stream.msg_id,
        message_thread_id: stream.thread_id,
        parse_mode: Some("MarkdownV2".to_string()),
        ..Default::default()
    };
    let chunks = split_message(&final_text, 4096);
    for chunk in &chunks {
        if let Ok(sent) = state.tg.send_message(chat_id, chunk, &send_opts).await {
            state.msg_sessions.insert(
                format!("{}:{}", chat_id, sent.message_id),
                session_id.to_string(),
            );
        }
    }

    // Clear reaction
    if let Some(msg_id) = stream.msg_id {
        let _ = state
            .tg
            .set_message_reaction(chat_id, msg_id, &[])
            .await;
    }

    // Drain pending queue: dispatch the next queued message for this session
    if let Some(queue) = state.pending_queue.get_mut(session_id) {
        if let Some(queued) = queue.pop_front() {
            if queue.is_empty() {
                state.pending_queue.remove(session_id);
            }
            let sid = session_id.to_string();
            message::dispatch_prompt(
                state,
                &queued.chat_id,
                queued.msg_id,
                queued.thread_id,
                queued.is_dm,
                &sid,
                queued.parts,
                queued.model,
            )
            .await;
        }
    }
}

async fn poll_approved(tg: &TelegramClient, approved_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(approved_dir);
    if let Ok(entries) = std::fs::read_dir(approved_dir) {
        for entry in entries.flatten() {
            if let Ok(chat_id) = std::fs::read_to_string(entry.path()) {
                let chat_id = chat_id.trim();
                if !chat_id.is_empty() {
                    let _ = tg
                        .send_message(
                            chat_id,
                            "You have been approved! You can now send me messages.",
                            &SendOpts::default(),
                        )
                        .await;
                }
            }
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
