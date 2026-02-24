# tauri-plugin-screenshot-hd

Pixel-perfect screenshot server for Tauri apps. One line to add, zero config needed.

## Why?

Tauri's WebDriver screenshot pipeline (used by `tauri-webdriver-automation` and `tauri-driver`) renders via SVG foreignObject → Canvas 2D. This **loses font rendering** — `@font-face` rules don't carry into the SVG's isolated data: URI context, and Canvas 2D uses a different text rasterizer than WebKit.

This plugin bypasses that entirely by calling `WKWebView.takeSnapshot` directly on macOS — the same rendering engine that displays the actual app. Fonts, antialiasing, CSS, and layout are pixel-perfect.

| | SVG foreignObject (WebDriver) | WKWebView.takeSnapshot (this plugin) |
|---|---|---|
| **Fonts** | Missing / fallback | Pixel-perfect |
| **CSS fidelity** | Most works, edge cases break | 100% — same engine |
| **Resolution** | 1x | Retina (2x on HiDPI) |
| **Platform** | Cross-platform | macOS only |

## Quick Start

### 1. Add the dependency

```toml
# Cargo.toml
[dependencies]
tauri-plugin-screenshot-hd = { git = "https://github.com/netbulls/tauri-plugin-screenshot-hd" }
```

### 2. Register the plugin

```rust
// src-tauri/src/lib.rs
let mut builder = tauri::Builder::default();

#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_screenshot_hd::init());
}
```

### 3. Take screenshots

```bash
# PNG bytes
curl -s http://127.0.0.1:21988/screenshot -o screenshot.png

# Run JS then screenshot
curl -s -X POST http://127.0.0.1:21988/eval?wait=500 \
  -d "document.querySelector('button').click()" \
  -o after-click.png

# Run JS only
curl -s -X POST http://127.0.0.1:21988/eval \
  -d "document.title = 'hello'"
```

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/screenshot` | Capture PNG via native WKWebView.takeSnapshot |
| `POST` | `/eval` | Execute JavaScript in the webview, returns `"ok"` |
| `POST` | `/eval?wait=<ms>` | Execute JS, wait N milliseconds, then return PNG |

## Configuration

Default: binds to `127.0.0.1:21988`, captures the `main` window.

```rust
use tauri_plugin_screenshot_hd::{init_with, Config};

builder = builder.plugin(init_with(Config {
    host: "127.0.0.1".into(),
    port: 9999,
    window_label: "popup".into(),
}));
```

## Build Modes

**Debug-only by default.** In release builds, `init()` is a no-op — no HTTP server, no attack surface.

To enable in release builds (e.g., for CI screenshots):

```toml
tauri-plugin-screenshot-hd = { git = "...", features = ["release"] }
```

## MCP Integration

Pair with [mcp-tauri-automation-hd](https://github.com/netbulls/mcp-tauri-automation-hd) to give AI agents (Claude Code) pixel-perfect screenshot capabilities:

```bash
claude mcp add --transport stdio tauri-automation-hd \
  --env TAURI_NATIVE_SCREENSHOT_URL=http://127.0.0.1:21988 \
  -- node /path/to/mcp-tauri-automation-hd/dist/index.js
```

The MCP server uses this plugin's `/screenshot` endpoint instead of the WebDriver's broken SVG pipeline.

## Platform Support

| Platform | Screenshot method | Status |
|----------|-------------------|--------|
| **macOS** | `WKWebView.takeSnapshot` | Supported |
| **Windows** | — | Returns error (use WebDriver fallback) |
| **Linux** | — | Returns error (use WebDriver fallback) |

On non-macOS platforms, `/screenshot` returns a 504 error explaining to use the WebDriver endpoint instead. The `/eval` endpoint works on all platforms.

## How It Works

On macOS, the plugin:

1. Gets the `WKWebView` pointer via `window.with_webview(|pv| pv.inner())`
2. Calls `takeSnapshotWithConfiguration:completionHandler:` via Rust's `objc` crate
3. Converts `NSImage` → TIFF → `NSBitmapImageRep` → PNG
4. Returns raw PNG bytes over HTTP

This is the same rendering path the OS uses to display the webview — no intermediate SVG, no Canvas 2D re-rendering.

## License

MIT
