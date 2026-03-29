# /claude Command — Claude Code Process Pool & TUI Skill System

## Overview

Add a `/claude` command to the Telegram bot that manages Claude Code interactive processes via PTY. Each `/claude` invocation spawns a new Claude Code process. Users interact with their sessions by replying to the bot's session message. The bot acts as a management layer — Claude Code handles user-facing chat via its own Telegram channel plugin (future userbot).

## Architecture

```
opencode-telegram-bot (@sh1marin_testing_bot)
  ├── Regular messages → opencode (zai-coding-plan/glm-5)
  └── /claude → spawn new Claude Code process (PTY)
        ├── TUI skill commands (via reply): /compact /clear /cost /model ...
        ├── Admin text injection (via reply): arbitrary input to PTY stdin
        └── Claude Code + Telegram userbot (future) → direct user interaction
```

## Process Pool

### Data Structures

```rust
struct ClaudeSession {
    pty_pair: PtyPair,           // portable-pty PTY pair
    child: Box<dyn Child>,       // claude child process
    vt_parser: vt100::Parser,    // virtual terminal emulator (ANSI → text grid)
    session_msg_id: i64,         // bot's session message ID (for reply routing)
    chat_id: String,             // originating Telegram chat
    user_id: String,             // originating user
    created_at: Instant,
    last_active: Instant,
    auth_state: AuthState,
}

enum AuthState {
    Unknown,
    WaitingForCode(String),      // OAuth URL, waiting for user to paste code
    LoggedIn,
}

struct ClaudePool {
    sessions: HashMap<i64, ClaudeSession>,  // session_msg_id → session
    max_total: usize,                        // global limit (e.g. 10)
}
```

### Session Lifecycle

1. **Creation**: `/claude` → allocate PTY (120x40) → spawn `claude --dangerously-skip-permissions` → handle trust dialog → check auth
2. **Interaction**: User replies to session message → route to session → inject via PTY → capture output → return to Telegram
3. **Cleanup**: 30-minute idle timeout → auto-kill. Bot restart → all sessions lost (PTY not persistent).

## /claude Command Behavior — Automatic State Machine

```
User sends /claude
  │
  ├── Pool full? → "Pool full, please try later"
  │
  ├── Spawn claude process in PTY
  │     ├── Trust dialog detected → send Enter
  │     ├── Check auth status:
  │     │     ├── Not logged in → auto-run `claude auth login`
  │     │     │     → extract OAuth URL (regex: https://claude.ai/oauth/...)
  │     │     │     → send URL to user
  │     │     │     → AuthState::WaitingForCode
  │     │     │     → user replies with code → inject to PTY → login completes
  │     │     │     → AuthState::LoggedIn → ready
  │     │     └── Already logged in → AuthState::LoggedIn → ready
  │     └── Send "🟢 Claude session started (id: xxx)" message
  │
  └── Save mapping: bot_message_id → session
```

## Session Interaction via Reply

All interaction with a session is done by replying to the bot's session message:

```
User: /claude
Bot: "🟢 Claude session abc123 started"  ← mapped to session

User (reply to above): /compact
Bot: → inject "/compact" to abc123 PTY → capture output → return result

User (reply to above): /cost
Bot: → inject "/cost" → parse TUI output via vt100 → format → return

User: /claude
Bot: "🟢 Claude session def456 started"  ← second session

User (reply to def456 message): /model sonnet
Bot: → inject to def456 PTY
```

Special commands:
- Reply with `/claude stop` → stop that session
- `/claude list` (not a reply) → list all active sessions for the user

## TUI Skill System

Each Claude TUI command needs a "skill" that knows how to:
1. Trigger the TUI operation via PTY input
2. Detect when the operation completes
3. Extract meaningful text from the TUI output
4. Format it for Telegram

### Core Skills

| Skill | PTY Input | Completion Detection | Output Parsing |
|-------|-----------|---------------------|----------------|
| `/compact` | `/compact\r` | Wait for prompt marker | "Conversation compacted" |
| `/clear` | `/clear\r` + confirm | Wait for prompt marker | "Conversation cleared" |
| `/cost` | `/cost\r` | Content stabilizes (3s) | Extract cost table from vt100 screen |
| `/model <name>` | `/model <name>\r` | Wait for prompt marker | "Model switched to ..." |
| `/help` | `/help\r` | Content stabilizes | Extract help text from vt100 screen |
| `/exit` | `/exit\r` | Process exits | "Session terminated" |
| Raw text | `<text>\r` | Content stabilizes (3s) | Extract response from vt100 screen |

### Response Completion Detection

Using `vt100::Parser` to maintain a virtual terminal screen:

1. Every 200ms: read PTY output → feed to vt100 parser
2. Compare `screen.contents()` with previous snapshot
3. If content unchanged for 3 consecutive seconds → response complete
4. OR: detect Claude's input prompt marker (`❯` or `>`) in the last line

### Output Extraction

`vt100::Parser` renders ANSI escape sequences into a text grid, solving the problem where Claude's TUI uses cursor movements (`\x1b[1C`) instead of space characters. `screen.contents()` returns clean text with proper spacing.

## Auth / Login Flow

```
/claude (first time, not logged in)
  → bot spawns claude → detects "not logged in" state
  → bot runs `claude auth login` in PTY
  → PTY output contains: "If the browser didn't open, visit: https://claude.ai/oauth/authorize?..."
  → bot extracts URL via regex, sends to Telegram user
  → user opens URL in browser → logs in → gets code from callback page
  → user replies to session message with the code
  → bot injects code into PTY stdin
  → claude completes login
  → bot confirms "✅ Logged in" → session ready
```

## Dependencies

### New Crates (Cargo.toml)
- `portable-pty = "0.9"` — Cross-platform PTY creation and management
- `vt100 = "0.16"` — Virtual terminal emulator for parsing ANSI output to text grid

### Existing (no changes needed)
- `tokio` (process, time, sync) — async runtime, already present
- `serde_json` — JSON handling, already present
- `regex` — pattern matching, already present

## File Changes

| File | Change | ~Lines |
|------|--------|--------|
| `Cargo.toml` | Add portable-pty, vt100 | +2 |
| `src/claude.rs` | **New** — ClaudePool, ClaudeSession, TUI skills, auth flow | ~300 |
| `src/message.rs` | Add /claude command routing, reply-to-session handling | ~50 |
| `src/main.rs` | `mod claude;`, BotState += claude_pool, stale session cleanup | ~15 |

## Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| Claude TUI layout changes across versions | vt100 screen parsing is layout-agnostic; only prompt detection regex needs updating |
| Response completion false positive (long thinking) | Use generous timeout (3s stable); provide manual "still waiting" retry |
| vt100 crate can't parse all Claude terminal sequences | vt100 is battle-tested (used by alacritty etc.); fallback to raw strip-ansi |
| PTY sessions lost on bot restart | Acceptable for now; document as known limitation |
| Concurrent process resource usage | Pool limit (max_total=10); idle timeout (30min); per-user limit possible |

## Future Work

- Telegram userbot integration for direct Claude ↔ user chat (bypassing the management bot)
- Session persistence across bot restarts (tmux fallback)
- Per-user session limits
- Cost tracking and budget enforcement via /cost skill
