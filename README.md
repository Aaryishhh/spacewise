# Spacewise

Storage intelligence for macOS and Windows: what's using your space, what's safe to
remove, and exactly what happens if you remove it.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the engine pipeline, repo layout,
and build-phase status. Currently at **Phase 1: architecture + repository structure**.

## Layout

- `crates/core` -- platform-agnostic Rust engine (scanner, classification, safety,
  recommendation, cleanup planning/execution, history)
- `crates/platform-{macos,windows,linux}` -- OS-specific adapters behind the
  `PlatformAdapter` trait
- `apps/desktop` -- Tauri + React/TypeScript desktop shell

## Development

Requires Rust (stable) and Node.js.

```sh
cargo check --workspace         # type-check the Rust workspace
cd apps/desktop && npm install  # install frontend deps
npm run tauri dev               # run the desktop app
```
