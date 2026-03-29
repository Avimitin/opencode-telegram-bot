# Codebase Analysis & Recommendations

This document outlines critical logical and architectural issues identified in the `opencode-telegram-bot` codebase and provides specific recommendations for improvement.

## 1. Critical Deadlock in Message Processing

### Issue
The bot's main loop in `src/main.rs` is sequential, creating a deadlock during message processing:
1. `main.rs` calls `message::handle_update` and `await`s it.
2. Inside `process_message` (called by `handle_update`), the code enters a loop that sleeps and waits for `active_streams[session_id].phase` to become `Phase::Done`.
3. However, the `Phase::Done` state is only updated by `process_sse_events`, which is called *after* `handle_update` returns in the main loop.

### Impact
The bot will hang for 300 seconds (the hardcoded timeout) every time a message is sent. During this hang, it will ignore all other Telegram updates and SSE events, effectively making the bot non-responsive.

### Recommendation
Refactor the message handling flow:
- Modify `process_message` to return immediately after initiating the `session_prompt` (via the existing `tokio::spawn`).
- Delegate the final message delivery (currently at the end of `process_message`) to the `process_sse_events` function when it receives a `session.idle` event.

---

## 2. Lack of SSE Connection Resilience

### Issue
The bot subscribes to the `opencode` SSE event stream exactly once at startup. If the `opencode` server restarts or the network connection drops, the stream returns `None` and the bot stops processing events.

### Impact
Once the connection is lost, the bot will stop providing streaming updates and will never finalize messages, requiring a manual restart of the bot itself.

### Recommendation
Implement a reconnection strategy in `src/main.rs`:
- Wrap the SSE subscription and the `process_sse_events` call in a loop.
- If the stream ends, wait for a few seconds and attempt to re-subscribe to the `opencode` server.

---

## 3. Fragile SSE Parsing

### Issue
In `src/sse.rs`, the parser strictly searches for `

` as the event separator.

### Impact
Standard SSE can use `

`, ``, or `

`. If the server or a proxy (like Nginx) uses `

`, the parser will fail to identify events or will leave dangling `` characters, potentially causing JSON parsing errors.

### Recommendation
Update `try_parse_event` to handle multiple line-ending formats or use a more robust approach (e.g., splitting by any sequence of `` and `
` that represents a blank line).

---

## 4. Ephemeral Session State

### Issue
The `msg_sessions` mapping (Telegram Message ID -> Opencode Session ID) is stored entirely in-memory using `BoundedMap`.

### Impact
If the bot restarts, all conversation contexts are lost. Users who reply to a bot message sent before the restart will have their message treated as a new session, breaking the continuity of the conversation.

### Recommendation
Persist the session mapping:
- Replace or augment `BoundedMap` with a simple persistent store (e.g., a JSON file or a lightweight database like `sled`).
- Load existing mappings on startup to maintain conversation continuity across bot restarts.

---

## 5. Inefficient Prefix Search

### Issue
`BoundedMap::find_last_by_prefix` performs a linear search through the `order` deque (up to 5,000 entries) on every new message that isn't a direct reply.

### Impact
While functional, this is an $O(N)$ operation that adds unnecessary latency as the map grows, especially on resource-constrained environments.

### Recommendation
- Optimization: Use a more efficient data structure or search strategy if prefix matching is a frequent operation. 
- Alternative: Maintain a secondary index for "last message in chat" to enable $O(1)$ lookups for session continuation.

---

## 6. Idiomatic Rust & Architecture Improvements

The current codebase follows a synchronous, imperative style that doesn't leverage Rust's safety and concurrency features effectively.

### High-Level Architecture: Actor-like Pattern
- **Current:** A single `loop` in `main.rs` manually polls Telegram, then calls a non-blocking SSE processor, then sleeps. This is fragile and hard to scale.
- **Recommendation:** Use `tokio::spawn` to run the Telegram Poller and the SSE Subscriber as independent tasks. Use an asynchronous channel (e.g., `tokio::sync::mpsc`) to communicate between them. This removes the need for manual `select!` timeouts and makes the bot significantly more responsive.

### Error Handling: `anyhow` or `thiserror`
- **Current:** Methods return `Result<T, String>`.
- **Recommendation:** Use `anyhow::Result<T>` for application-level errors or define a custom error enum with `thiserror`. Returning `String` loses type context and makes programmatic error handling difficult.

### State Management: `Arc<RwLock<...>>`
- **Current:** `BotState` is passed around as a monolithic `&mut BotState`.
- **Recommendation:** Wrap shared state in `Arc<RwLock<BotState>>`. This allows multiple tasks (Telegram poller, SSE handler, API workers) to access the state safely and concurrently, which is essential for a high-performance bot.

### Data Types: Enums and Typestates
- **Current:** Heavy reliance on string matching (e.g., `if part_type == "reasoning"`).
- **Recommendation:** 
  - Parse SSE data into strongly-typed enums immediately upon receipt.
  - Use the **Typestate Pattern** for `StreamState` to ensure at compile-time that illegal state transitions (e.g., appending text to a finished stream) are impossible.

### Functional Iterators
- **Current:** Manual `for` loops with `push` and `join` for string building.
- **Recommendation:** Use iterator chains (`map`, `filter`, `collect`) and `format_args!` to improve both performance and readability.

### XML Prompt Construction
- **Current:** Manual XML sanitization and string concatenation.
- **Recommendation:** Use a dedicated XML builder or a templating engine (like `tera`) for prompt construction to ensure injection safety and maintainability as prompt complexity grows.
