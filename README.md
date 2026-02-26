# Darling
(🚧 WIP) 

## LLM (Unified Interface)

Darling exposes a single Tauri command interface that can talk to multiple LLM protocols:

- `llm_prompt(prompt: string) -> string` (uses env vars; easiest for the Capsule UI)
- `llm_chat(request) -> { text, raw }` (fully specified provider + messages)

## Brain (Context + Intent + Prompt Assembly)

MVP (no memory yet): `brain_run` captures best-effort local context when you trigger the capsule (`//`),
then assembles a system prompt and calls the configured LLM.

- Command: `brain_run({ input }) -> { text }`
- Context currently includes: interrupted app name / bundle id / pid, focused window title, best-effort selected text (Accessibility), plus a short clipboard snapshot.
- Optional (opt-in): screenshot + "screen context" extraction (separate vision step; gives intent-ish image context):
  - `DARLING_CTX_SCREENSHOT=1`
  - `DARLING_VISION_EXTRACT=1`
  - `DARLING_VISION_MODEL=gpt-5.2`
  - `DARLING_VISION_API_KEY=...`

## Paste / Injection

By default Darling does **not** overwrite your clipboard: it injects text by typing Unicode events into the previously active app.

- Configure via `.env`: `DARLING_INJECT_MODE=unicode | clipboard_restore | clipboard`

### Quick setup (OpenAI-compatible)

Create a `.env` (local file, gitignored) or export env vars before running `npm run tauri dev`:

- `DARLING_LLM_KIND=openai_compat`
- `DARLING_LLM_MODEL=gpt-4o-mini` (example)
- `DARLING_LLM_API_KEY=...`
- `DARLING_LLM_BASE_URL=https://api.openai.com/v1` (optional; defaults to OpenAI)

### Other providers

- Anthropic:
  - `DARLING_LLM_KIND=anthropic`
  - `DARLING_LLM_MODEL=claude-3-5-sonnet-20241022` (example)
  - `DARLING_LLM_API_KEY=...`
  - `DARLING_LLM_BASE_URL=https://api.anthropic.com` (optional)
- Ollama (local):
  - `DARLING_LLM_KIND=ollama`
  - `DARLING_LLM_MODEL=llama3.2` (example)
  - `DARLING_LLM_BASE_URL=http://localhost:11434` (optional)
- Gemini:
  - `DARLING_LLM_KIND=gemini`
  - `DARLING_LLM_MODEL=gemini-1.5-flash` (example)
  - `DARLING_LLM_API_KEY=...`
  - `DARLING_LLM_BASE_URL=https://generativelanguage.googleapis.com` (optional)
