# Architecture

opencode-telegram-bot is a Telegram frontend for [opencode](https://opencode.ai). It bridges Telegram messages to opencode's LLM session API, with streaming output, tool call display, and MarkdownV2 formatting.

## Components

```
┌─────────────────────────────────────────────────────────┐
│                    Telegram Users                        │
│              (DMs and Group messages)                    │
└──────────────────────┬──────────────────────────────────┘
                       │ Telegram Bot API (long polling)
                       ▼
┌──────────────────────────────────────────────────────────┐
│                opencode-telegram-bot                      │
│                                                          │
│  ┌──────────┐  ┌──────────┐  ┌───────────┐              │
│  │ config   │  │ access   │  │ session   │              │
│  │ .env,    │  │ gate,    │  │ tracking, │              │
│  │ paths    │  │ pairing, │  │ queues    │              │
│  │          │  │ mentions │  │           │              │
│  └──────────┘  └──────────┘  └───────────┘              │
│                                                          │
│  ┌──────────┐  ┌──────────┐  ┌───────────┐              │
│  │ message  │  │ stream   │  │ markdown  │              │
│  │ handle,  │  │ SSE sub, │  │ Md2       │              │
│  │ prompt   │  │ display  │  │ convert   │              │
│  └──────────┘  └──────────┘  └───────────┘              │
│                                                          │
│  ┌──────────┐  ┌──────────┐                              │
│  │ models   │  │ download │                              │
│  │ picker   │  │ files    │                              │
│  └──────────┘  └──────────┘                              │
└──────────────────────┬──────────────────────────────────┘
                       │ HTTP (localhost)
                       ▼
┌──────────────────────────────────────────────────────────┐
│              opencode serve (child process)               │
│                                                          │
│  - Spawned by SDK: createOpencode({ port: 0 })           │
│  - Manages sessions, prompts, tool execution             │
│  - Streams events via SSE                                │
│  - Stores state in $HOME/.local/share/opencode/          │
└──────────────────────┬──────────────────────────────────┘
                       │ HTTPS (via proxy if configured)
                       ▼
┌──────────────────────────────────────────────────────────┐
│                   LLM Provider API                        │
│            (e.g. Zhipu, OpenAI, Anthropic)               │
└──────────────────────────────────────────────────────────┘
```

## Source Modules

| Module | Responsibility |
|---|---|
| `config.ts` | Load .env file, export paths (STATE_DIR, ACCESS_FILE, etc.) and TELEGRAM_BOT_TOKEN |
| `access.ts` | Access control types, cached loadAccess/saveAccess (2s TTL), pairing flow, gate check, mention pattern matching |
| `session.ts` | Bounded Maps for session tracking (msgSessions, msgModelOverride, max 5000 entries), per-chat message queue |
| `stream.ts` | StreamState type, SSE subscriber that listens to opencode events and drives streaming display |
| `markdown.ts` | Convert LLM markdown to Telegram MarkdownV2 (code blocks, bold, italic, links, inline code) |
| `models.ts` | Fetch and cache model list from opencode, paginated inline keyboard for model selection |
| `download.ts` | Download Telegram file attachments, convert to base64 data URLs for opencode |
| `message.ts` | Main message handling: command parsing, gate check, session resolution, prompt construction, streaming lifecycle, final message send |
| `index.ts` | Entry point: start opencode server, create bot, register handlers, start polling |

## Message Flow

1. **Receive**: grammy receives a Telegram message (text, photo, document, etc.)
2. **Gate**: Check if the sender is allowed (group policy / DM allowlist / pairing)
3. **Queue**: Messages are queued per-chat to prevent concurrent prompts to the same session
4. **Session**: Resolve or create an opencode session (reply to bot → continue session, @mention in group → new session)
5. **Prompt**: Wrap user text in `<channel>` XML with metadata (chat_id, user, timestamp), attach files as base64
6. **Stream**: Fire prompt via SDK, SSE subscriber receives streaming events:
   - `reasoning` → display with 💭 prefix
   - `text` → display as-is
   - `tool` → display tool calls with 🔧 prefix
   - Throttled updates: 1.5s for groups (Telegram rate limit), 0.3s for DMs
7. **Finalize**: Delete streaming placeholder, send final message in MarkdownV2 format

## Access Control

- **DMs**: Controlled by `dmPolicy` — "pairing" (new users get a code to approve), "allowlist" (only pre-approved), or "disabled"
- **Groups**: Per-group policy with `requireMention` (bot only responds when @mentioned or replied to) and optional `allowFrom` user whitelist
- **Mention detection**: Checks `mentionPatterns` from access.json plus the bot's own @username (auto-detected at startup)

## State

All mutable state lives in the `stateDir` (default `/var/lib/opencode-telegram`):

```
stateDir/
├── opencode.json                          # LLM provider config (written by preStart)
├── .opencode/channels/telegram/
│   ├── access.json                        # Access control config (written by preStart)
│   ├── .env                               # TELEGRAM_BOT_TOKEN (written by preStart)
│   └── approved/                          # Pairing approval files (polled at runtime)
└── .local/share/opencode/
    ├── opencode-stable.db                 # Session/message database (managed by opencode)
    └── log/                               # opencode server logs
```

In-memory state (not persisted, lost on restart):
- `msgSessions`: Map of bot reply message IDs → session IDs (max 5000)
- `msgModelOverride`: Map of model-set confirmation messages → model IDs (max 5000)
- `activeStreams`: Map of session IDs → streaming state (cleaned up after each prompt)
- `chatQueues`: Per-chat promise chains for sequential processing
