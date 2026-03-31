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

9. **Return errors to callers, don't silently log** — functions should return `Result`, not swallow errors with `eprintln!` and return defaults. Let the caller decide how to handle it (e.g. show error to the user in Telegram).
   **Why:** Silent failures hide problems from users who can't access server logs. Errors should surface to where they can be acted on.
   **How to apply:** Any function that catches errors internally and returns empty/default values.

10. **Return borrowed slices instead of cloning** — prefer returning `&[T]` over `Vec<T>` when the data is owned by the callee. Let callers `.to_vec()` if they truly need ownership.
    **Why:** Avoids unnecessary allocations and clones. Most callers only need to iterate or pass references.
    **How to apply:** Any getter/cache method that returns a collection from an owned field.

11. **Use file mtime for cache invalidation, not TTL** — when caching file contents, compare the file's modification time to decide whether to reload, not a fixed time interval.
    **Why:** mtime reloads exactly when the file changes — no stale data, no unnecessary re-reads.
    **How to apply:** Any cache backed by a file on disk.

12. **Don't cache with TTL when data is static** — if the underlying data doesn't change during the process lifetime (e.g. model list from a long-running server), fetch once and cache permanently. No TTL needed.
    **Why:** TTL implies the data might change, which is misleading and adds pointless complexity.
    **How to apply:** Any cache where the data source is immutable for the process lifetime.

13. **Eliminate dead fields — don't keep unused struct fields** — if a struct field is never read, remove it entirely rather than suppressing the warning with `#[allow(dead_code)]`.
    **Why:** Dead fields mislead readers into thinking the field is used somewhere, add unnecessary allocations at construction sites, and accumulate as dead weight. Removing them keeps the codebase honest.
    **How to apply:** When adding a field, wire it through immediately. If a refactor leaves a field unread, delete it and all construction-site references.

14. **Group related parameters into semantic structs, declare new types proactively** — when a function takes multiple parameters that belong to the same concept (e.g. Telegram message identity, chat context, session reference), introduce a named struct for that group. Don't wait for clippy's `too_many_arguments` lint — proactively recognize when arguments form a coherent unit and extract them. Aim to keep function signatures at 4 or fewer parameters.
    **Why:** Named types make call sites self-documenting and let the compiler enforce correctness. Readers understand `PromptParams { chat_id, session_id, parts, .. }` at a glance but struggle with positional `(chat_id, msg_id, thread_id, is_dm, session_id, parts, model)`. Extracting early avoids painful refactors later as parameters grow.
    **How to apply:** When designing a new function or adding a parameter to an existing one, look at the argument list and ask: "do any of these always travel together?" If yes, create a struct (e.g. `ChatContext<'a>`, `PromptParams<'a>`, `MessageRef`) with public fields. Use `let FooParams { a, b, .. } = params;` destructuring inside the function body.

15. **Collapse nested `if let` / `if` guards into let-chains** — when an `if let` body contains another `if` condition, combine them with `&&` into a single let-chain.
    **Why:** Rust 2024 edition supports let-chains natively. Nesting adds indentation and an extra block for no benefit — the collapsed form expresses the single guard condition more clearly.
    **How to apply:** `if let Ok(x) = expr { if x.is_valid() { ... } }` becomes `if let Ok(x) = expr && x.is_valid() { ... }`.

16. **Remove needless borrows — let the compiler auto-ref** — don't write `&value` at a call site when the function takes `&T` and `value` is already `T`. Let Rust's auto-ref coerce the reference.
    **Why:** Explicit `&` where auto-ref handles it adds visual noise and clippy flags it as needless_borrow.
    **How to apply:** Pass the owned value directly; only use `&` when the value would otherwise be moved and is needed later.

17. **Simplify trivially identical branches** — if an `if`/`else` produces the same value on both sides, replace the entire expression with that value directly.
   **Why:** Identical branches are dead logic — they suggest a missing distinction or a copy-paste mistake. Removing them removes ambiguity for the reader.
   **How to apply:** `if cond { x } else { x }` → `x`. If the condition was meant to differentiate, fix the values instead.

18. **Inline trivial single-use wrappers** — if a function is called exactly once and its body is 1–2 straightforward lines (e.g. a method call plus a constructor), inline the logic at the call site and delete the function. Don't wrap simple operations in named functions that add indirection without abstraction value.
   **Why:** A trivial wrapper called once adds a name readers must track and a jump they must follow, for zero reuse benefit. Inlining keeps the logic visible where it matters.
   **How to apply:** When a function has a single call site and its body is trivial, move the body to the caller and remove the function definition.

20. **Run `nix develop --command cargo clippy` before reporting work is done** — after every code change, run clippy (via nix dev shell) and fix any warnings or errors before considering the task complete.
   **Why:** Catching issues early prevents regressions from accumulating. Clippy enforces idiomatic Rust and catches common mistakes that compile but are wrong or misleading.
   **How to apply:** Make it the last step of every code change — modify code, run `nix develop --command cargo clippy`, fix findings, repeat until clean.

19. **Don't introduce blocks to scope bindings unnecessarily** — prefer plain sequential `let` statements over `let x = { ... }` blocks unless the block has a semantic purpose (e.g. borrowing a field with temporary scope, or a mutex lock that must drop). Don't wrap code in blocks just to eagerly drop an intermediate variable.
   **Why:** Extra blocks add nesting and visual noise for no real gain — the intermediate binding is harmless in the outer scope. Reserve blocks for cases where the scope truly matters (lock guards, shadowing, lifetime control).
   **How to apply:** `let x = { let y = foo(); Bar(y) };` → `let y = foo(); let x = Bar(y);`. Only use a block if `y` holds a resource (lock, file handle) whose early drop is intentional and documented.
