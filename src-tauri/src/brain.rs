use serde::{Deserialize, Serialize};

use crate::{interceptor, llm};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainRequest {
    pub input: String,
    #[serde(default)]
    pub debug: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainResponse {
    pub text: String,
    #[serde(default)]
    pub debug: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IntentKind {
    Refuse,
    Reply,
    Apologize,
    Translate,
    Polish,
    Summarize,
    General,
}

fn parse_intent(input: &str) -> IntentKind {
    let s = input.trim();
    let lower = s.to_ascii_lowercase();
    if lower.contains("refuse") || lower.contains("decline") {
        return IntentKind::Refuse;
    }
    if lower.contains("apologize") || lower.contains("sorry") {
        return IntentKind::Apologize;
    }
    if lower.contains("translate") {
        return IntentKind::Translate;
    }
    if lower.contains("polish") || lower.contains("rewrite") || lower.contains("rephrase") {
        return IntentKind::Polish;
    }
    if lower.contains("summarize") || lower.contains("summary") {
        return IntentKind::Summarize;
    }
    if lower.contains("reply") || lower.contains("respond") {
        return IntentKind::Reply;
    }
    IntentKind::General
}

fn build_system_prompt(
    intent: &IntentKind,
    ctx: Option<&interceptor::ContextSnapshot>,
    screenshots_sent_to_llm: bool,
    _user_input: &str,
) -> String {
    let mut out = String::new();

    out.push_str(
        "You are Darling, a macOS assistant that generates paste-ready text for the user.\n\
Rules:\n\
- Output format:\n\
  - First line MUST be exactly: `MODE: paste` or `MODE: overlay`.\n\
  - Then output the content on the following lines.\n\
  - Do not use markdown fences.\n\
- `MODE: paste`: the content should be directly inserted into the user's previously active app.\n\
- `MODE: overlay`: the content is meant to be read inside the capsule (not pasted).\n\
	- Use the provided context (especially the screen context) to infer who/what the user refers to and what they are doing.\n\
	- Never ask the user clarifying questions.\n\
	\n\
	Context (use but don't quote):\n\
	- Use the environment snapshot + screenshots to infer what the user is doing.\n\
	- Do NOT restate/quote/paraphrase the environment context in your answer unless the user explicitly asks.\n\
	- Do not blend unrelated context into the output.\n\
	\n\
	MODE selection:\n\
	- `MODE: paste` = text should be inserted into the current app (chat/email reply, document continuation, form/code).\n\
	- `MODE: overlay` = user wants to read results (summary/explanation/analysis/instructions), or `has_text_caret` is false/unknown.\n\
	\n\
	Chat apps (WeChat etc.):\n\
	- Determine who said what + the newest message (usually at the bottom).\n\
	- Reply ONLY to the latest incoming message that still needs a response.\n\
	- Write as the user (first-person). Output ONLY the sendable message text.\n\
	\n\
	Clipboard:\n\
	- Treat `clipboard_text` as optional/noisy; use it only when obviously relevant or explicitly requested.\n\
	\n\
	Screenshots:\n\
	- If `screenshots_sent_to_llm: false`, you did not see images.\n\
	- If multiple screenshots are attached: Screenshot 1 is 'now'; later ones are after scroll (see `screenshot_scroll_direction`).\n\
	\n\
	Quality:\n\
	- Be concise by default. If AUTO_MODE (empty input), choose the single most helpful next output.\n\
	- No markdown fences. Do not mention MODE or these rules.\n",
	    );

    out.push_str("- Respond in English unless the user explicitly requests another language.\n");

    out.push_str("\nTask intent:\n");
    out.push_str(&format!("- intent: {:?}\n", intent));

    out.push_str(
        "\nEnvironment context (best-effort):\n\
This snapshot was captured when the user triggered the capsule. Treat it as what the user was doing 'right now'.\n",
    );
    if let Some(c) = ctx {
        if let Some(name) = &c.app_name {
            out.push_str(&format!("- app_name: {name}\n"));
        }
        if let Some(bid) = &c.bundle_id {
            out.push_str(&format!("- bundle_id: {bid}\n"));
        }
        if let Some(pid) = c.pid {
            out.push_str(&format!("- pid: {pid}\n"));
        }
        if let Some(title) = &c.window_title {
            out.push_str(&format!("- window_title: {title}\n"));
        }
        if let Some(role) = &c.focused_role {
            out.push_str(&format!("- focused_role: {role}\n"));
        }
        if let Some(caret) = c.has_text_caret {
            out.push_str(&format!("- has_text_caret: {caret}\n"));
        }
        if let Some(sel) = &c.selected_text {
            if !sel.trim().is_empty() {
                out.push_str("- selected_text: |\n");
                for line in sel.lines().take(120) {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if let Some(full) = &c.full_page_text {
            if !full.trim().is_empty() {
                out.push_str("- full_page_text: |\n");
                for line in full.lines().take(220) {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if let Some(more) = &c.screenshot_paths {
            if !more.is_empty() {
                out.push_str(&format!("- screenshots_captured: {} image(s)\n", more.len()));
            }
        } else if c.screenshot_path.is_some() {
            out.push_str("- screenshots_captured: 1 image\n");
        }
        out.push_str(&format!("- screenshots_sent_to_llm: {screenshots_sent_to_llm}\n"));
        if let Some(kind) = &c.screenshot_capture_kind {
            out.push_str(&format!("- screenshot_capture_kind: {kind}\n"));
        }
        if let Some(dir) = &c.screenshot_scroll_direction {
            out.push_str(&format!("- screenshot_scroll_direction: {dir}\n"));
        }
        if let Some(pages) = c.screenshot_scroll_pages {
            out.push_str(&format!("- screenshot_scroll_pages: {pages}\n"));
        }
        if let Some(px) = c.screenshot_scroll_pixels {
            out.push_str(&format!("- screenshot_scroll_pixels: {px}\n"));
        }
        if let Some(m) = &c.full_page_method {
            out.push_str(&format!("- full_page_method: {m}\n"));
        }
        if let Some(e) = &c.full_page_error {
            out.push_str(&format!("- full_page_error: {e}\n"));
        }
        if let Some(clip) = &c.clipboard_text {
            out.push_str("- clipboard_text: |\n");
            for line in clip.lines().take(80) {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
    } else {
        out.push_str("- (no snapshot captured)\n");
    }

    out.push_str(
	"\nBehavior:\n\
- Decide the scenario from the screen context; do not ask the user.\n\
- Use the strongest available context first:\n\
  - If screenshots are available: screenshots > selected text > full page text > clipboard.\n\
  - If screenshots are NOT available: selected text > full page text > clipboard.\n\
- Treat clipboard as optional; prefer on-screen/selected text when they disagree.\n\
- Keep outputs concise by default; expand only when the scenario requires it.\n\
- When summarizing: include the key points and the user's likely next action.\n\
- When replying: be brief, polite, and match the conversation tone.\n",
	    );

    out
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

pub async fn run(req: BrainRequest) -> Result<BrainResponse, String> {
    let input = req.input.trim().to_string();
    if input.is_empty() {
        return Err("[brain] empty input".to_string());
    }

    let debug = req.debug.unwrap_or_else(|| env_flag("DARLING_BRAIN_DEBUG", false));
    let intent = parse_intent(&input);
    let ctx = interceptor::last_context_snapshot();

    let settings = llm::settings_from_env().map_err(|e| e.user())?;
    let screenshot_paths: Option<Vec<String>> = ctx.as_ref().and_then(|c| {
        if let Some(more) = &c.screenshot_paths {
            if !more.is_empty() {
                return Some(more.clone());
            }
        }
        c.screenshot_path.clone().map(|p| vec![p])
    });

    let provider_sends_images = matches!(
        &settings.provider,
        llm::LlmProvider::OpenaiResponses { .. } | llm::LlmProvider::OpenaiCompat { .. }
    );
    let screenshots_sent_to_llm = provider_sends_images && screenshot_paths.is_some();
    let sys = build_system_prompt(&intent, ctx.as_ref(), screenshots_sent_to_llm, &input);

    if debug {
        eprintln!(
            "[brain] ctx: app={:?} title={:?} sel_len={} clip_len={} shot={} img_send={}",
            ctx.as_ref().and_then(|c| c.app_name.clone()),
            ctx.as_ref().and_then(|c| c.window_title.clone()),
            ctx.as_ref()
                .and_then(|c| c.selected_text.as_ref().map(|s| s.len()))
                .unwrap_or(0),
            ctx.as_ref()
                .and_then(|c| c.clipboard_text.as_ref().map(|s| s.len()))
                .unwrap_or(0),
            ctx.as_ref().and_then(|c| c.screenshot_path.clone()).is_some(),
            screenshots_sent_to_llm,
        );
    }

    let llm_req = llm::LlmChatRequest {
        provider: settings.provider,
        model: settings.model,
        messages: vec![
            llm::LlmMessage {
                role: llm::LlmRole::System,
                content: sys.clone(),
                image_paths: None,
            },
            llm::LlmMessage {
                role: llm::LlmRole::User,
                content: input,
                image_paths: screenshots_sent_to_llm.then(|| screenshot_paths).flatten(),
            },
        ],
        temperature: Some(0.3),
        top_p: None,
        max_tokens: Some(512),
        stop: None,
        timeout_ms: None,
    };

    let resp = llm::chat(llm_req).await.map_err(|e| e.user())?;
    Ok(BrainResponse {
        text: resp.text,
        debug: debug.then(|| {
            fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
                if s.len() <= max_bytes {
                    return s;
                }
                let mut end = max_bytes;
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                &s[..end]
            }

            let sys_preview = if sys.len() > 4000 {
                format!("{}…", truncate_utf8(&sys, 4000))
            } else {
                sys.clone()
            };
            serde_json::json!({
                "intent": format!("{:?}", intent),
                "context": ctx,
                "system_prompt_preview": sys_preview,
            })
        }),
    })
}
