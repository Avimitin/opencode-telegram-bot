---
name: Rust code conventions
description: Code style and patterns to follow when writing Rust code in this project
type: feedback
---

When writing or modifying Rust code, follow these conventions:

1. **Use `anyhow` for error handling** — don't use `Box<dyn Error>` or `Result<T, String>`. Use `anyhow::Result<T>` with `Context` / `with_context` for adding context to errors.
   **Why:** anyhow provides better error chains, backtraces, and a cleaner API than manual string formatting.
   **How to apply:** All functions that return Result. Use `.context("description")` instead of `.map_err(|e| format!("...: {}", e))`.

2. **Never convert errors to strings** — don't use `map_err(|e| format!(...))` or `map_err(|e| e.to_string())`. Use anyhow's `Context` trait to wrap errors with context while preserving the original error chain.
   **Why:** String conversion loses the original error type and prevents downcasting. Context preserves the full chain.
   **How to apply:** Replace all `map_err(|e| format!("foo: {}", e))` with `.context("foo")`.

3. **Use `error_for_status()` for HTTP responses** — don't manually check `resp.status().is_success()` and format error strings. Use reqwest's `error_for_status()` chained with `.context()`.
   **Why:** Less boilerplate, and the reqwest error includes the status code automatically.
   **How to apply:** Any HTTP client code that checks response status.

4. **Use multi-thread tokio runtime** — don't use `#[tokio::main(flavor = "current_thread")]`. Use the default `#[tokio::main]` (multi-thread) so spawned tasks can run on worker threads.
   **Why:** current_thread serializes all I/O on one thread, meaning slow HTTP calls in spawned tasks block the main loop.
   **How to apply:** The tokio::main attribute on the entry point.

5. **Use `anyhow::Result` as main return type** — return `anyhow::Result<()>` from main and use `?` instead of match/exit(1) patterns.
   **Why:** Reduces boilerplate and provides consistent error reporting.
   **How to apply:** The main function signature and all startup initialization that can fail.

6. **Follow XDG base directory spec for config paths** — use `$XDG_CONFIG_HOME` (fallback `$HOME/.config`) for config files. Always provide an env var override for paths.
   **Why:** Respects user's directory layout preferences, and env var overrides are essential for deployment flexibility.
   **How to apply:** Any code that locates config/state files.

7. **Keep latest edition** — use Rust edition 2024. Handle reserved keywords (e.g. `r#gen`) and new safety requirements (e.g. `unsafe` for `env::set_var`).
   **Why:** Access to latest language features like let-chains, and forward compatibility.
   **How to apply:** Cargo.toml edition field.

8. **Disable proxy for localhost clients** — when creating reqwest clients that only talk to localhost, use `Client::builder().no_proxy().build()`.
   **Why:** Avoids routing local traffic through system proxy, which would fail or add unnecessary latency.
   **How to apply:** Any HTTP client used exclusively for local service communication.
