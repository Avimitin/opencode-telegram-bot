# Project Conventions

Follow the conventions in `.claude/` when writing code for this project:

- [Rust conventions](.claude/feedback_rust_conventions.md) — anyhow errors, no string conversion, XDG paths, multi-thread tokio, no_proxy for localhost
- [Nix conventions](.claude/feedback_nix_conventions.md) — import over legacyPackages, rec attrsets, mandatory options, writeText for configs, version assertions

## Building

Always run `cargo` inside the nix dev shell to ensure the project-local toolchain and dependencies are used consistently, regardless of what the host has installed.

```
nix develop --command cargo check    # type-check
nix develop --command cargo build    # debug build
nix develop --command cargo clippy   # lint
nix develop --command cargo run      # start bot
```
