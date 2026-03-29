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
use crate::message::BotState;
use crate::models::ModelCache;
use crate::opencode::{OpencodeClient, OpencodeServer};
use crate::session::BoundedMap;
use crate::sse::SseStream;
use crate::stream::Phase;
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
        ])
        .await;

    // Subscribe to SSE events
    let sse_response = match oc.event_subscribe().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to subscribe to SSE: {}", e);
            std::process::exit(1);
        }
    };
    let mut sse_stream = SseStream::new(sse_response);
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
        bot_id: me.id,
        bot_username,
    };

    println!("Telegram bot @{} is running!", me.username.as_deref().unwrap_or("?"));

    // Main loop: poll Telegram updates and process SSE events
    let mut update_offset: i64 = 0;
    let mut last_approved_poll = std::time::Instant::now();

    loop {
        // Poll for Telegram updates (short timeout to interleave SSE processing)
        match state.tg.get_updates(update_offset, 1).await {
            Ok(updates) => {
                for update in &updates {
                    update_offset = update.update_id + 1;
                    message::handle_update(&mut state, update).await;
                }
            }
            Err(e) => {
                eprintln!("getUpdates error: {}", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }

        // Process SSE events (non-blocking)
        process_sse_events(&mut state, &mut sse_stream).await;

        // Poll approved pairing directory every 5 seconds
        if last_approved_poll.elapsed().as_secs() >= 5 {
            last_approved_poll = std::time::Instant::now();
            poll_approved(&state.tg, &config.approved_dir).await;
        }
    }
}

async fn process_sse_events(state: &mut BotState, sse: &mut SseStream) {
    // Process available SSE events (non-blocking via tokio::select with timeout)
    loop {
        let event = tokio::select! {
            e = sse.next_event() => e,
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => return,
        };

        let event = match event {
            Some(e) => e,
            None => return,
        };

        if event.event_type == "message.part.updated" {
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
                            let _ = state
                                .tg
                                .edit_message_text(&chat_id, stream_msg_id, &display)
                                .await;
                        }
                        stream.mark_updated();
                    }
                }
            }
        }

        if event.event_type == "session.idle" {
            let session_id = event
                .data
                .get("properties")
                .and_then(|p| p.get("sessionID"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(stream) = state.active_streams.get_mut(session_id) {
                if stream.phase != Phase::Done {
                    stream.phase = Phase::Done;
                }
            }
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
