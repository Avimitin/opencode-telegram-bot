# Development & Implementation Details

## Why a local opencode server?

The bot doesn't call LLM APIs directly. Instead, it spawns a local `opencode serve` process via the `@opencode-ai/sdk`. This gives us:

- Session management, tool execution, and permission handling for free
- Streaming via SSE events
- Model/provider configuration through opencode's existing config system
- The same LLM interaction layer that the opencode CLI/TUI uses

The SDK calls `spawn("opencode", ["serve", ...])` and communicates over HTTP on localhost. The opencode binary must be in PATH — the nix package wrapper handles this via `makeWrapper --prefix PATH`.

## Why cached access control?

`loadAccess()` reads `access.json` from disk. In the original single-file implementation, this was called 3+ times per message (in `stripMention`, `gate`, and `processMessage`). We now cache with a 2-second TTL to avoid redundant disk reads while still picking up changes (e.g. when the admin approves a pairing).

Writes via `saveAccess()` update the cache immediately so the caller sees their own changes.

## Why bounded Maps?

`msgSessions` and `msgModelOverride` map bot message IDs to session IDs. Without bounds, these grow forever as the bot handles messages. We cap at 5000 entries with FIFO eviction — old enough entries are unlikely to be replied to.

`activeStreams` is self-cleaning (entries are deleted after each prompt completes), so it doesn't need bounds.

## Why Unicode PUA for markdown placeholders?

`toMarkdownV2` needs to protect inline code, links, and bold/italic markers from being escaped. The original implementation used `\x00` (null bytes) as sentinel placeholders. This is fragile — if LLM output contains null bytes (unlikely but possible with certain encodings), the substitution breaks silently.

We use Unicode Private Use Area codepoints (`\uE000`–`\uE007`) instead. These are guaranteed to never appear in normal text and are safe in UTF-8 strings.

## Why auto-detect bot username for mentions?

The `mentionPatterns` in `access.json` is configured by the deployer. If they set it to `["@MyBot"]` but the actual bot username is `@my_test_bot` (e.g. different token for testing), mention detection silently fails and the bot responds to everything or nothing.

`getMentionPatterns()` now always includes the bot's own `@username` (from `bot.botInfo.username`) alongside configured patterns. This makes it work out of the box without manual configuration.

## Why lazy botInfo access?

grammy's `bot.botInfo` is only available after `bot.init()` or `bot.start()`. The message handler registration happens before `bot.start()`, so accessing `bot.botInfo.id` at registration time throws. We use lazy getters (`getBotInfoId()`, `getBotUsername()`) that read `bot.botInfo` at call time.

## Why input sanitization in the prompt?

User text is embedded in an XML-like `<channel>` wrapper:

```xml
<channel source="telegram" chat_id="..." user="..." ts="...">
  user text here
</channel>
```

Without sanitization, a user could inject `</channel>` to break out of the wrapper. `sanitizeForXml()` escapes `&`, `<`, `>` to HTML entities. This is defense-in-depth — the LLM shouldn't execute injected instructions, but the XML structure should still be well-formed.

## Why per-chat message queues?

opencode sessions are stateful — sending two prompts concurrently to the same session causes race conditions. `enqueue()` chains messages per chat into a sequential promise queue, ensuring only one prompt is active per chat at a time.

## Why a fixed-output derivation for node_modules?

Nix builds run in a sandbox with no network access. `bun install` needs to download packages from the registry. A fixed-output derivation (FOD) is the standard nix escape hatch — it allows network access during build but requires a pre-declared content hash. If the output doesn't match the hash, the build fails.

The hash is determined by building once with an empty hash (nix reports the actual hash in the error), then filling it in. When dependencies change (new packages in bun.lock), the hash must be updated.

## Why makeWrapper for the binary?

The bot needs two runtime dependencies not captured by bun's module resolution:

1. **opencode binary** — the SDK spawns it as a child process via `spawn("opencode", ...)`
2. **bun runtime** — to execute the TypeScript source

`makeWrapper` creates a shell script that sets up PATH before executing bun. This way the systemd service only needs `ExecStart = ${package}/bin/opencode-telegram-bot` — all runtime dependencies are part of the nix closure.

## Proxy and localhost

When `http_proxy` is set, bun's `fetch()` routes **all** HTTP requests through the proxy, including `http://127.0.0.1`. The bot communicates with the local opencode server over HTTP on localhost. If this traffic goes through an external proxy, the proxy can't reach the local server and returns 503.

The NixOS module should ideally set `no_proxy=127.0.0.1,localhost` automatically when proxy variables are present. Currently this must be configured by the deployer via `extraEnvironment`. See the proxy section in [deploy.md](deploy.md).

## Debugging tips

- **opencode server logs**: `$HOME/.local/share/opencode/log/` — check the latest .log file for server-side errors
- **opencode API path**: The API uses `/session`, `/session/prompt`, etc. — NOT `/api/session`. The `/api/` prefix returns the SPA HTML.
- **Lazy bootstrap**: opencode server returns 503 until the first real API request triggers project initialization. This can take several seconds.
- **Empty SDK errors**: `result.error = {}` usually means the HTTP response wasn't valid JSON (proxy error, HTML fallback, connection refused).
