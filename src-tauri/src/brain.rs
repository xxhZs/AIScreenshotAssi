use serde::{Deserialize, Serialize};

use crate::{interceptor, llm, vision};

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
    if s.contains("拒绝") || s.contains("婉拒") {
        return IntentKind::Refuse;
    }
    if s.contains("道歉") || s.contains("抱歉") {
        return IntentKind::Apologize;
    }
    if s.contains("翻译") || s.contains("译成") {
        return IntentKind::Translate;
    }
    if s.contains("润色") || s.contains("改写") {
        return IntentKind::Polish;
    }
    if s.contains("总结") || s.contains("概括") {
        return IntentKind::Summarize;
    }
    if s.contains("回复") || s.contains("回他") || s.contains("回她") {
        return IntentKind::Reply;
    }
    IntentKind::General
}

fn looks_like_zh(input: &str) -> bool {
    // Heuristic: any CJK char => zh.
    input.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
}

fn build_system_prompt(
    intent: &IntentKind,
    ctx: Option<&interceptor::ContextSnapshot>,
    user_input: &str,
) -> String {
    let mut out = String::new();

    out.push_str(
        "You are Darling, a macOS assistant that generates paste-ready text for the user.\n\
Rules:\n\
- Output ONLY the text to paste. No explanations, no markdown fences.\n\
- Use the provided context (especially the screen context) to infer who/what the user refers to and what they are doing.\n\
- If context is insufficient, ask ONE short clarifying question instead of guessing.\n",
    );

    if looks_like_zh(user_input) {
        out.push_str("- Respond in Simplified Chinese unless the user explicitly requests another language.\n");
    } else {
        out.push_str("- Respond in the user's language.\n");
    }

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
        if let Some(clip) = &c.clipboard_text {
            out.push_str("- clipboard_text: |\n");
            for line in clip.lines().take(80) {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
        if let Some(sc) = &c.ocr_text {
            if !sc.trim().is_empty() {
                out.push_str("- screen_context_from_screenshot: |\n");
                for line in sc.lines().take(120) {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    } else {
        out.push_str("- (no snapshot captured)\n");
    }

    out.push_str(
        "\nBehavior:\n\
- Decide the most likely scenario yourself based on the screen context and the user's instruction.\n\
- Examples of scenarios: replying in a chat, continuing a document, drafting an email, writing code, fixing an error, filling a form.\n\
- Match tone/length to the scenario (chat replies are short; documents are coherent and longer; code is precise).\n\
- If the user's instruction is vague, use the screen context to choose the best next output instead of asking a generic question.\n",
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
    let mut ctx = interceptor::last_context_snapshot();
    let mut vision_error: Option<String> = None;
    if let Some(c) = ctx.as_mut() {
        // Screenshot extraction is intentionally a separate model (vision) step,
        // so the main text-generation LLM can remain a black box.
        if c.ocr_text.is_none() {
            let mut paths: Vec<String> = Vec::new();
            if let Some(more) = c.screenshot_paths.clone() {
                paths.extend(more);
            } else if let Some(p) = c.screenshot_path.clone() {
                paths.push(p);
            }
            if !paths.is_empty() {
                match vision::extract_screen_context_from_screenshot(&paths, Some(c), &input).await {
                    Ok(Some(text)) => c.ocr_text = Some(text),
                    Ok(None) => {}
                    Err(e) => {
                        if debug {
                            eprintln!("[brain] vision extract error: {e}");
                        }
                        vision_error = Some(e);
                    }
                }
            }
        }
    }

    let sys = build_system_prompt(&intent, ctx.as_ref(), &input);
    if debug {
        eprintln!(
            "[brain] ctx: app={:?} title={:?} sel_len={} clip_len={} shot={} ocr_len={}",
            ctx.as_ref().and_then(|c| c.app_name.clone()),
            ctx.as_ref().and_then(|c| c.window_title.clone()),
            ctx.as_ref()
                .and_then(|c| c.selected_text.as_ref().map(|s| s.len()))
                .unwrap_or(0),
            ctx.as_ref()
                .and_then(|c| c.clipboard_text.as_ref().map(|s| s.len()))
                .unwrap_or(0),
            ctx.as_ref().and_then(|c| c.screenshot_path.clone()).is_some(),
            ctx.as_ref()
                .and_then(|c| c.ocr_text.as_ref().map(|s| s.len()))
                .unwrap_or(0),
        );
    }

    let settings = llm::settings_from_env().map_err(|e| e.user())?;
    let llm_req = llm::LlmChatRequest {
        provider: settings.provider,
        model: settings.model,
        messages: vec![
            llm::LlmMessage {
                role: llm::LlmRole::System,
                content: sys.clone(),
            },
            llm::LlmMessage {
                role: llm::LlmRole::User,
                content: input,
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
            let sys_preview = if sys.len() > 4000 {
                format!("{}…", &sys[..4000])
            } else {
                sys.clone()
            };
            serde_json::json!({
                "intent": format!("{:?}", intent),
                "context": ctx,
                "vision_error": vision_error,
                "system_prompt_preview": sys_preview,
            })
        }),
    })
}
