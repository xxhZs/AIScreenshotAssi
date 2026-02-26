# Darling

Darling is a global, floating AI assistant for macOS: type a trigger in any app to open a lightweight capsule, press Enter, and it generates text you can immediately use—then returns focus back to your app.

Great for:
- Chat/email/IM replies: generate a natural response in one go
- Documents: continue writing, rewrite, or summarize what’s on your screen
- Coding: generate code or suggest the next fix based on what you’re looking at

## Features

- Global summon: no need to switch to a main window
- Paste-ready output: inserts text back into your app (best-effort)
- Screen context (optional): capture screen context so the model can adapt to what you’re doing

## Requirements

- macOS (recommended 13+)
- Node.js + npm
- Rust toolchain（Tauri）

On first run you’ll need to grant system permissions (see below).

## Install & Run (dev)

1) Install dependencies

```bash
npm install
```

2) Configure `.env`

Copy the template:

```bash
cp .env.example .env
```

In `.env`, configure at least one text model (the main model):
- `DARLING_LLM_KIND` (usually `openai_compat`)
- `DARLING_LLM_MODEL`
- `DARLING_LLM_API_KEY`
- `DARLING_LLM_BASE_URL` (if you use a proxy / OpenAI-compatible gateway)

3) Start

```bash
npm run tauri dev
```

## Permissions (important)

Darling needs system permissions to support global summon and input injection:

- **Accessibility**: global trigger + reading selected text/window info
- **Input Monitoring**: intercepting the trigger
- (Optional) **Screen Recording**: screenshots for screen context

Go to: System Settings → Privacy & Security → enable Darling in the relevant sections.

## Usage

1) In any app, type the trigger: `//`
2) Type what you want and press Enter
3) Darling generates output and either pastes it back or shows it in the capsule, depending on the scenario

Examples:
- “Draft a polite decline.”
- “Summarize what I’m looking at into 3 bullets.”
- “Given this error, what should I change next?”

### Debug mode (optional)

Press `Cmd+Shift+D` in the capsule to open a debug panel showing captured context and run details.

## Screen context (optional)

If you want the model to adapt to what you’re doing, enable screenshot context:

Set in `.env`:
- `DARLING_CTX_SCREENSHOT=1`

If you’re in a browser (Safari/Chrome) and want “full page text” when possible, enable full-page capture (preferred over screenshots when it works):
- `DARLING_CTX_FULLPAGE=1`

If content is behind scrollbars and screenshots are incomplete, you can enable multi-shot scrolling capture (it scrolls briefly and then scrolls back):
- `DARLING_CTX_SCROLL_CAPTURE=1`
 - Direction: `DARLING_CTX_SCROLL_DIRECTION=down|up|both`

### Dedicated vision model (recommended)

To keep the main model as a “black box” text model, Darling can use a separate vision model to convert screenshots into a compact “screen context”, then feed that into the main model.

Example `.env` settings:
- `DARLING_VISION_EXTRACT=1`
- `DARLING_VISION_MODEL=gpt-5.2`
- `DARLING_VISION_API_KEY=...`
- `DARLING_VISION_BASE_URL=...`

## Build

```bash
npm run tauri build
```

Build artifacts will be produced under Tauri’s output directory.

## Security & privacy

- Enabling screenshot context means screenshots may be sent to your configured vision provider (if enabled).
- Use only providers/proxies you trust, and avoid enabling it on sensitive screens.
