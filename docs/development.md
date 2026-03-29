# Development Notes

## Debugging: `Failed to create session` under systemd (2026-03-29)

### Symptom

Bot replies "Failed to create session. Please try again." to every message when running under systemd, but works fine when run directly with `bun run src/index.ts`.

### Root Cause

The systemd service had `http_proxy` and `https_proxy` environment variables set for accessing external APIs (Telegram, LLM providers). Bun's `fetch()` honors these proxy variables for **all** HTTP requests, including requests to `http://127.0.0.1` (the local opencode server).

The SDK creates a local opencode server via `createOpencode({ port: 0 })`, which spawns `opencode serve` on a random localhost port. When the SDK client calls the opencode API (e.g. `session.create`), `fetch()` routes the request through the proxy instead of connecting directly to localhost. The proxy cannot reach `127.0.0.1` on the host machine, so it returns 503.

### Fix

Add `no_proxy=127.0.0.1,localhost` to the service environment so localhost requests bypass the proxy.

### Debugging Timeline

1. **Initial observation**: `Session create error: {}` — empty error object, unhelpful.
2. **Checked opencode binary**: `opencode --version` works, `opencode serve` starts normally.
3. **Tested SDK client manually**: `createOpencodeClient({ baseUrl: "http://127.0.0.1:4096" })` with `bun -e` worked — sessions created successfully. This was misleading because the manual test ran without `http_proxy` set.
4. **Added raw fetch debugging**: Direct `fetch()` from within the bot process returned HTTP 503 — confirmed the issue was not SDK-specific.
5. **Checked opencode server logs**: The server showed NO incoming requests despite the bot sending them — requests were being intercepted before reaching the server.
6. **Tested curl with proxy**: `http_proxy=http://... curl http://127.0.0.1:4096/...` returned empty/error — confirmed the proxy was the culprit.

### Lessons

- When `http_proxy` is set, **all** HTTP traffic goes through it, including localhost. Always set `no_proxy=127.0.0.1,localhost` alongside proxy variables.
- Empty error objects (`{}`) from the SDK usually mean the HTTP response was not valid JSON (e.g. proxy error page, HTML SPA fallback).
- The opencode API path is `/session`, not `/api/session.create`. The `/api/` prefix returns the SPA HTML, which is easy to mistake for a broken API.
- The opencode server does lazy bootstrapping — it only initializes the project on the first API request, not at startup. The `opencode server listening` log line does not mean the server is ready to handle requests.

## Mention Detection

The bot uses `mentionPatterns` from `access.json` to detect when it's being addressed in group chats. The bot's own `@username` is now automatically included in the pattern list (via `getMentionPatterns()`), so it works regardless of whether `access.json` has the correct username configured.

## Nix Packaging

The bot is packaged as a nix derivation (`nix/package.nix`):

- **node_modules**: Fetched as a fixed-output derivation (FOD) using `bun install --frozen-lockfile`. The output hash must be updated when dependencies change — build with an empty hash to get the correct one.
- **Bin wrapper**: `makeWrapper` creates `bin/opencode-telegram-bot` that sets `PATH` to include the `opencode` binary and runs `bun run <store-path>/src/index.ts`.
- **NixOS module** (`nix/module.nix`): Uses `ExecStart = "${cfg.package}/bin/opencode-telegram-bot"` — all dependencies are captured in the nix closure, no need for `path = [ ... ]` in the systemd service.

### Deploying without NixOS

For machines that aren't managed by NixOS, `deploy.sh` evaluates the NixOS module to generate a systemd unit file:

```sh
nix build .#nixosConfigurations.eval.config.systemd.units.'"opencode-telegram.service"'.unit
```

This builds all dependencies (opencode binary, bun, bot package) and produces a ready-to-install service unit file.
