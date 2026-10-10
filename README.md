# mdpeek

A tiny, fast Markdown viewer for Windows — Notepad-simple on the outside, full rendering on the inside.

[中文说明](README.zh-CN.md)

![light](docs/screenshots/light-main.png)
![dark](docs/screenshots/dark-main.png)

## Features

- GFM rendering: headings, tables, task lists, strikethrough, links, images
- Links never take over the window: clicking one (or `Ctrl + click` / middle-click) hands it to the system, `http(s)` to your default browser, a local path to its file association. In-page `#anchors` still scroll in place
- Local images just work: relative paths, subdirectories, `../`, absolute Windows paths and `file:///` are all resolved against the document's own folder
- Mermaid diagrams rendered from ` ```mermaid ` code blocks
- Syntax highlighting via highlight.js
- Light / dark theme that follows the system, one key to toggle
- Live reload — the view refreshes itself when the file changes on disk (pair it with Notepad for a zero-friction edit + preview loop)
- Outline sidebar with scroll-spy highlighting
- Source view: peek at the raw Markdown with one key
- Built-in editor: edit the Markdown in place and save with `Ctrl + S` — no round-trip to Notepad
- Toolbar under the title bar: editor / source / outline / on-top / theme / zoom / print, one click away
- Always-on-top, zoom, print / export to PDF, custom context menu
- Single ~5 MB exe, no installer, no Electron — Rust + system WebView2

## Usage

```
mdpeek.exe path\to\file.md
```

Or just drag a `.md` file onto the window.

## Keyboard & menu

Everything is also on the toolbar and in the right-click menu.

| Key | Action |
|---|---|
| `Ctrl + E` | Toggle the editor |
| `Ctrl + S` | Save (while editing) |
| `Ctrl + O` | Toggle outline sidebar |
| `Ctrl + M` | Toggle Markdown source view |
| `Ctrl + B` | Reveal file in Explorer |
| `Ctrl + R` | Reload file |
| `Ctrl + T` | Toggle light / dark theme |
| `Ctrl + P` | Print / export to PDF |
| `Ctrl + 0` / `Ctrl + =` / `Ctrl + -` | Reset / zoom in / zoom out |
| `Ctrl + wheel` | Zoom |
| `Esc` | Leave the editor, close the menu, or collapse the outline |

Opening the file in Notepad is still available from the right-click menu.

## File association

```
mdpeek.exe --register     :: make mdpeek the default handler for .md / .markdown (per-user, no admin)
mdpeek.exe --unregister   :: undo it
```

The exe registers its own absolute path, so run it from the folder where you intend to keep it.

## Build

```
cargo build --release
```

Output: `target\release\mdpeek.exe` — a single self-contained binary. Requires the WebView2 runtime (preinstalled on Windows 10/11).

## Stack

Rust · [wry](https://github.com/tauri-apps/wry) + [tao](https://github.com/tauri-apps/tao) · WebView2 · marked · mermaid · highlight.js

## License

MIT — see [LICENSE](LICENSE).
