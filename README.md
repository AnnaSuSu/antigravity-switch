# Antigravity Switch

Lightweight cross-platform (macOS & Windows) menubar/tray popover app for Google Antigravity, built with **Tauri v2 (Rust + React/TypeScript)**.

Zero `app.asar` patching. Zero official binary modifications. Runs in the background without occupying the macOS Dock, with native Liquid Glass (`NSGlassEffectView` / `NSVisualEffectView`) and content-adaptive popover height.

## Features

- **Multi-Account Switching**: Import current logged-in account or add accounts via Google OAuth2 loopback login. One-click switch syncs macOS Keychain / Windows Credential Manager, `state.vscdb`, and `~/.gemini/` CLI credentials, restarts `language_server`, verifies active email via `GetUserStatus`, and rolls back automatically on failure.
- **Auto-Switch**: Monitor a chosen quota pool (`5h` / `7d` / tightest window) and automatically switch to the highest-quota candidate when remaining quota drops below a threshold (lazy on-demand candidate checks, zero idle Google requests).
- **Quota Dashboard & Live Menubar**: Shared quota pools, remaining percentages, reset countdowns, and configurable menubar summary (`quota% · context · tok/s`).
- **Session Context & Speed**: Connects to local Antigravity LanguageServer (`~/.gemini/antigravity/daemon/hub_*.json`) to inspect active/recent conversation context window usage, input/cache/output/thinking breakdown, TTFT, and streaming `tok/s`.
- **Token Usage History**: Scans local Desktop and CLI transcripts (with read-only WAL-safe SQLite model backfill) for today / 7d / 30d / all-time usage, cache hit ratio, daily trend bars, and per-model breakdown.
- **No-TUN Proxy Launcher**: Configure HTTP/HTTPS/SOCKS5 proxy, test connectivity, and launch or restart Antigravity with `HTTP_PROXY`/`HTTPS_PROXY` and `--proxy-server` without touching `app.asar`.
- **Menubar Popover UX**: Hidden from the macOS Dock (`Accessory` policy), multi-monitor & fullscreen Space aware, auto-hide on focus loss (with optional pin toggle), and silent startup via `--silent`.

## Development

Prerequisites: Node.js 22+, Rust (stable).

```bash
npm install
npm run tauri dev
```

Build release bundle:

```bash
npm run tauri build
```

Run Rust unit tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

## License

MIT
