import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow, currentMonitor, primaryMonitor } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";

/**
 * Capsule — the core AI input pill.
 *
 * Flow:
 *   1. User types a prompt and hits Enter.
 *   2. Invoke Tauri command `brain_run_async` → external runtime returns result.
 *   3. Show result inside the capsule.
 */
export default function Capsule({ onDismiss, context }) {
  const inputRef = useRef(null);
  const textareaRef = useRef(null);
  const contentRef = useRef(null);
  const overlayTextRef = useRef(null);
  const inputPanelRef = useRef(null);
  const overlayPanelRef = useRef(null);
  const debugPanelRef = useRef(null);
  const winRef = useRef(null);
  const resizeRafRef = useRef(null);
  const lastAppliedHeightRef = useRef(null);
  const lastAppliedWidthRef = useRef(null);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [showDebug, setShowDebug] = useState(false);
  const [brainDebug, setBrainDebug] = useState(null);
  const [lastResult, setLastResult] = useState(null);
  const [lastEngine, setLastEngine] = useState(null);
  const [lastInvokeError, setLastInvokeError] = useState(null);
  const [overlayOutput, setOverlayOutput] = useState(null);
  const [copied, setCopied] = useState(false);
  const [currentJobId, setCurrentJobId] = useState(null);
  const currentJobIdRef = useRef(null);
  const [toast, setToast] = useState(null);
  const [lastRunDir, setLastRunDir] = useState(null);

  // Auto-focus every time the capsule mounts (i.e. every time it becomes visible).
  useEffect(() => {
    inputRef.current?.focus();
  }, []);
  useEffect(() => {
    if (context) console.log("[Capsule] context snapshot:", context);
  }, [context]);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 900);
    return () => clearTimeout(t);
  }, [copied]);
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 1200);
    return () => clearTimeout(t);
  }, [toast]);

  useEffect(() => {
    let unlisten = null;
    listen("job_done", (event) => {
      const payload = event?.payload;
      if (!payload) return;
      if (!currentJobIdRef.current || payload.job_id !== currentJobIdRef.current) {
        return;
      }

      setCurrentJobId(null);
      currentJobIdRef.current = null;
      if (payload.run_dir) setLastRunDir(payload.run_dir);
      if (payload.debug) setBrainDebug(payload.debug);
      if (!payload.ok) {
        const errMsg = payload.error || "Job failed";
        setLastInvokeError(errMsg);
        setOverlayOutput(errMsg);
        setLoading(false);
        return;
      }
      setToast("任务完成");
      applyResult(payload.text || "").catch((err) => {
        console.error("[Capsule] applyResult failed:", err);
        setOverlayOutput(String(err?.message ?? err));
        setLoading(false);
      });
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      if (unlisten) unlisten();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const panelWidth = "w-full";

  const scheduleResize = () => {
    if (resizeRafRef.current) cancelAnimationFrame(resizeRafRef.current);
    resizeRafRef.current = requestAnimationFrame(() => {
      resizeToContent().catch(() => {});
    });
  };

  const scheduleResizeBurst = () => {
    scheduleResize();
    setTimeout(() => scheduleResize(), 30);
    setTimeout(() => scheduleResize(), 160);
  };

  const clamp = (n, min, max) => Math.max(min, Math.min(max, n));

  const resizeToContent = async () => {
    if (!contentRef.current) return;

    const win = winRef.current ?? getCurrentWindow();
    winRef.current = win;

    const monitor =
      (await currentMonitor().catch(() => null)) ??
      (await primaryMonitor().catch(() => null));
    const factor =
      monitor?.scaleFactor ?? (await win.scaleFactor().catch(() => 1));

    const workW = monitor?.workArea?.size?.width;
    const workH = monitor?.workArea?.size?.height;
    const workWLogical = typeof workW === "number" ? workW / factor : 1200;
    const workHLogical = typeof workH === "number" ? workH / factor : 900;

    // Target width: readable but not huge.
    const desiredWidth = clamp(Math.floor(workWLogical - 56), 420, 760);
    const maxAllowedHeight = clamp(Math.floor(workHLogical * 0.85), 260, 1100);

    // Ensure the overlay output never pushes content below the window bounds:
    // - When output is short, grow the window so it's fully visible.
    // - When output is long, cap the window height and make only the output box scroll.
    const contentEl = contentRef.current;
    const overlayTextEl = overlayTextRef.current;
    if (overlayTextEl) {
      if (overlayOutput) {
        // Use the current DOM to compute how much height is available for the output box,
        // rather than relying on hard-coded "chrome" estimates (which can clip content).
        //
        // `fixed` is everything in the capsule except the output text box itself
        // (padding, input panel, overlay header/buttons/footer, debug panel, gaps, etc).
        const fixed = contentEl.scrollHeight - overlayTextEl.offsetHeight;
        const availableForText = clamp(Math.floor(maxAllowedHeight - fixed), 140, maxAllowedHeight);
        overlayTextEl.style.maxHeight = `${availableForText}px`;
      } else {
        overlayTextEl.style.maxHeight = "";
      }
    }
    // Force a reflow so `scrollHeight` reflects the latest maxHeight.
    // (Important right after output renders, to prevent "cut off" windows.)
    void contentEl.getBoundingClientRect();

    let desiredHeight = clamp(Math.ceil(contentEl.scrollHeight), 200, maxAllowedHeight);

    if (lastAppliedHeightRef.current != null && lastAppliedWidthRef.current != null) {
      const hSame = Math.abs(lastAppliedHeightRef.current - desiredHeight) < 8;
      const wSame = Math.abs(lastAppliedWidthRef.current - desiredWidth) < 4;
      if (hSame && wSame) return;
    }
    lastAppliedHeightRef.current = desiredHeight;
    lastAppliedWidthRef.current = desiredWidth;

    try {
      await win.setSize(new LogicalSize(desiredWidth, desiredHeight));
      // Extra ticks help settle layout and avoid truncation.
      scheduleResizeBurst();
    } catch (e) {
      // Ignore resize failures (e.g., if the window is hidden or constrained).
      console.warn("[Capsule] window resize failed:", e);
    }
  };

  // Auto-resize textarea height (multi-line) for better readability.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "0px";
    const max = 160; // ~8-9 lines
    const next = Math.min(el.scrollHeight, max);
    el.style.height = `${next}px`;
    el.style.overflowY = el.scrollHeight > max ? "auto" : "hidden";
    scheduleResize();
  }, [query]);

  // Auto-resize the Tauri window to fit content (best-effort).
  useEffect(() => {
    const el = contentRef.current;
    if (!el) return;
    scheduleResize();
    if (typeof ResizeObserver === "undefined") {
      return () => {
        if (resizeRafRef.current) cancelAnimationFrame(resizeRafRef.current);
      };
    }
    const ro = new ResizeObserver(() => scheduleResize());
    ro.observe(el);
    return () => {
      ro.disconnect();
      if (resizeRafRef.current) cancelAnimationFrame(resizeRafRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    scheduleResizeBurst();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [overlayOutput, showDebug, loading]);

  const runBrain = async (input) => {
    const resp = await invoke("brain_run", {
      request: { input, debug: showDebug },
    });
    setLastEngine("brain_run");
    setLastInvokeError(null);
    setBrainDebug(resp?.debug ?? null);
    if (resp?.debug) console.log("[Capsule] brain debug:", resp.debug);
    const result = resp?.text ?? "";
    if (!result) throw new Error("brain_run returned empty text");
    return result;
  };

  const startBackgroundRun = async (input) => {
    const jobId = await invoke("brain_run_async", {
      request: { input, debug: showDebug },
    });
    setCurrentJobId(jobId);
    currentJobIdRef.current = jobId;
    setLastEngine("brain_run_async");
    setLastInvokeError(null);
  };

  const parseMode = (raw) => {
    const s = (raw ?? "").trimStart();
    const lines = s.split("\n");
    const first = (lines[0] ?? "").trim();
    if (first === "MODE: paste") {
      return { mode: "paste", content: lines.slice(1).join("\n").trim() };
    }
    if (first === "MODE: overlay") {
      return { mode: "overlay", content: lines.slice(1).join("\n").trim() };
    }
    // Fallback if provider didn't follow the contract.
    return { mode: "paste", content: s.trim() };
  };

  const applyResult = async (result) => {
    const { mode, content } = parseMode(result);
    const trimmed = (content ?? "").trim();
    const caretState = context?.has_text_caret;

    if (!trimmed) {
      setOverlayOutput("(empty response)");
      setLastResult("");
      setQuery("");
      queueMicrotask(() => inputRef.current?.focus());
      setLoading(false);
      return;
    }

    // Always show results in the capsule (no auto-paste).
    if (true) {
      setOverlayOutput(trimmed);
      setLastResult(trimmed);
      setQuery("");
      queueMicrotask(() => inputRef.current?.focus());
      scheduleResizeBurst();
      setLoading(false);
      return;
    }
  };

  const ctxFlags = (() => {
    const flags = [];
    if (context?.selected_text) flags.push("sel");
    if (context?.clipboard_text) flags.push("clip");
    if (context?.window_title) flags.push("title");
    if (context?.full_page_text) flags.push("fullpage");
    if (context?.screenshot_path) flags.push("shot");
    if (context?.has_text_caret !== true) flags.push("no-caret");
    return flags;
  })();

  const handleKeyDown = async (e) => {
    // Cmd+Shift+D toggles a small debug panel showing captured context.
    if (e.key?.toLowerCase?.() === "d" && e.metaKey && e.shiftKey) {
      e.preventDefault();
      setShowDebug((v) => !v);
      return;
    }

    // Shift+Enter inserts a newline in the textarea.
    if (e.key !== "Enter" || e.shiftKey) return;
    if (loading) return;

    e.preventDefault();
    setLoading(true);

    try {
      try {
        const current = query.trim();
        const autoSeed =
          "AUTO_MODE: The user pressed Enter with empty input. Based on the screen context, generate the single most appropriate next paste-ready output for what they are doing right now.";

        // Starting a new run clears any previous overlay content.
        if (overlayOutput) setOverlayOutput(null);
        await startBackgroundRun(current || autoSeed);
        return;
      } catch (e) {
        console.warn("[Capsule] brain_run_async failed, falling back to brain_run:", e);
        setLastEngine("brain_run_failed");
        setLastInvokeError(String(e?.message ?? e));
        setBrainDebug(null);
        const current = query.trim();
        const autoSeed =
          "AUTO_MODE: The user pressed Enter with empty input. Based on the screen context, generate the single most appropriate next paste-ready output for what they are doing right now.";
        const result = await runBrain(current || autoSeed);
        await applyResult(result);
        return;
      }
    } catch (err) {
      console.error("[Capsule] runtime error:", err);
    } finally {
      // Loading is cleared when the job finishes.
    }
  };

  return (
    /*
      Outer wrapper — full viewport, transparent, click-through on empty space.
      The pill itself is centered and ~580 px wide, Spotlight-style.
    */
    <div
      className="w-full h-full"
      onMouseDown={(e) => {
      if (e.target === e.currentTarget) onDismiss();
      }}
    >
      <div ref={contentRef} className="p-4 flex flex-col items-center gap-3 antialiased w-full">
	      <div
          ref={inputPanelRef}
	        className={[
	          // ── shape ──────────────────────────────────────────────────────────
	          "flex flex-col",
	          panelWidth,
	          "px-4 py-3 rounded-2xl",
	          // ── solid background (readability) ────────────────────────────────
	          "bg-black",
	          // ── border + shadow ────────────────────────────────────────────────
	          "border border-white/14",
	          "shadow-[0_18px_70px_rgba(0,0,0,0.85)]",
	          // ── text ───────────────────────────────────────────────────────────
	          "text-white",
	          // ── transition ─────────────────────────────────────────────────────
	          "transition-all duration-150",
	        ].join(" ")}
	      >
        <div className="flex items-start gap-3">
          {/* Leading icon — magnifier, swapped for a spinner while loading */}
          <span className="text-white/55 select-none shrink-0 mt-[2px]">
            {loading ? (
              <svg
                className="animate-spin h-5 w-5 text-white/65"
                xmlns="http://www.w3.org/2000/svg"
                fill="none"
                viewBox="0 0 24 24"
              >
                <circle
                  className="opacity-25"
                  cx="12"
                  cy="12"
                  r="10"
                  stroke="currentColor"
                  strokeWidth="4"
                />
                <path
                  className="opacity-75"
                  fill="currentColor"
                  d="M4 12a8 8 0 018-8v8H4z"
                />
              </svg>
            ) : (
              <svg
                className="h-5 w-5 text-white/55"
                xmlns="http://www.w3.org/2000/svg"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M21 21l-4.35-4.35M17 11A6 6 0 1 1 5 11a6 6 0 0 1 12 0z"
                />
              </svg>
            )}
          </span>

          {/* Main text input (auto-growing textarea) */}
          <textarea
            ref={(el) => {
              textareaRef.current = el;
              inputRef.current = el;
            }}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={loading}
            placeholder={
              context?.app_name
                ? `What should I do? (Current: ${context.app_name})`
                : "What should I do?"
            }
	            className={[
	              "flex-1 w-full bg-transparent outline-none border-none",
	              "resize-none",
	              "text-[16px] leading-[1.50] font-medium tracking-[-0.012em]",
	              "text-white placeholder-white/50 caret-white",
	              "disabled:opacity-50",
	              "min-h-[28px]",
	            ].join(" ")}
            rows={1}
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
          />
        </div>

        <div className="mt-2 flex items-center justify-between gap-3 text-[11px] text-white/35 select-none">
          <span>
            ↵ {query.trim() ? "run" : "auto"} · ⇧↵ newline
          </span>
          <span className="truncate">
            {context?.app_name ? context.app_name : ""}
            {ctxFlags.length ? ` · ${ctxFlags.join("·")}` : ""}
            {toast ? ` · ${toast}` : ""}
          </span>
        </div>
      </div>

      {overlayOutput && (
        <div
          ref={overlayPanelRef}
          className={[
            panelWidth,
            "rounded-2xl border border-white/12",
            // Same look as the input panel: solid background for readability.
            "bg-black",
            "shadow-[0_18px_70px_rgba(0,0,0,0.85)]",
            "px-4 py-3",
            "flex flex-col min-h-0",
          ].join(" ")}
        >
          <div className="flex items-center justify-between gap-3">
            <div className="text-[11px] uppercase tracking-wide text-white/45">
              Result
            </div>
            <div className="flex items-center gap-2">
              <button
                className={[
                  "text-[11px] px-2 py-1 rounded-md",
                  "border border-white/12",
                  "bg-white/5 hover:bg-white/10",
                  "text-white/75 hover:text-white/90",
                ].join(" ")}
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(overlayOutput);
                    setCopied(true);
                  } catch (e) {
                    console.warn("[Capsule] clipboard copy failed:", e);
                  }
                }}
              >
                {copied ? "Copied" : "Copy"}
              </button>
              <button
                className={[
                  "text-[11px] px-2 py-1 rounded-md",
                  "border border-white/12",
                  "bg-white/5 hover:bg-white/10",
                  "text-white/60 hover:text-white/90",
                ].join(" ")}
                onClick={() => {
                  setOverlayOutput(null);
                  setLastResult(null);
                  scheduleResize();
                }}
              >
                Clear
              </button>
            </div>
          </div>
          <div
            ref={overlayTextRef}
            className={[
              "mt-3",
              "text-[15px] leading-[1.75] tracking-[-0.004em]",
              "text-white select-text whitespace-pre-wrap",
              "rounded-xl bg-white/5 border border-white/10",
              "px-3 py-2",
              "overflow-auto pr-1",
              "min-h-0",
            ].join(" ")}
          >
            {overlayOutput}
          </div>
          <div className="mt-3 text-[11px] text-white/35 select-none">Esc to close</div>
        </div>
      )}

      {showDebug && (
        <pre
          ref={debugPanelRef}
          className={[
            panelWidth,
            "max-h-[300px] overflow-auto",
            "rounded-xl border border-white/15",
            "bg-black",
            "px-4 py-3 text-[11px] leading-relaxed",
            "text-white/80",
            "select-text",
          ].join(" ")}
        >
          {JSON.stringify(
            {
              layout_debug: {
                viewport: {
                  width: typeof window !== "undefined" ? window.innerWidth : null,
                  height: typeof window !== "undefined" ? window.innerHeight : null,
                },
                content: {
                  scroll_height: contentRef.current?.scrollHeight ?? null,
                  offset_height: contentRef.current?.offsetHeight ?? null,
                },
                input_panel: {
                  offset_height: inputPanelRef.current?.offsetHeight ?? null,
                },
                overlay: overlayOutput
                  ? {
                      text_max_height_style: overlayTextRef.current?.style?.maxHeight ?? null,
                      text_offset_height: overlayTextRef.current?.offsetHeight ?? null,
                      text_scroll_height: overlayTextRef.current?.scrollHeight ?? null,
                      panel_offset_height: overlayPanelRef.current?.offsetHeight ?? null,
                    }
                  : null,
              },
              show_capsule_context: context ?? null,
              last_brain_debug: brainDebug ?? null,
              last_result_preview: lastResult ?? null,
              last_engine: lastEngine ?? null,
              last_invoke_error: lastInvokeError ?? null,
              last_run_dir: lastRunDir ?? null,
            },
            null,
            2
          )}
        </pre>
      )}
      </div>
    </div>
  );
}
