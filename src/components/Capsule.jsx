import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";

/**
 * Capsule — the core AI input pill.
 *
 * Flow:
 *   1. User types a prompt and hits Enter.
 *   2. Invoke Tauri command `query_memory` → get response string from Rust.
 *   3. Emit `inject_text` → Rust writes to clipboard and pastes into the
 *      active application via simulated Cmd+V.
 *   4. Dismiss the capsule.
 */
export default function Capsule({ onDismiss, context }) {
  const inputRef = useRef(null);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [showDebug, setShowDebug] = useState(false);
  const [brainDebug, setBrainDebug] = useState(null);
  const [lastResult, setLastResult] = useState(null);
  const [lastEngine, setLastEngine] = useState(null);
  const [lastInvokeError, setLastInvokeError] = useState(null);

  // Auto-focus every time the capsule mounts (i.e. every time it becomes visible).
  useEffect(() => {
    inputRef.current?.focus();
  }, []);
  useEffect(() => {
    if (context) console.log("[Capsule] context snapshot:", context);
  }, [context]);

  const ctxFlags = (() => {
    const flags = [];
    if (context?.selected_text) flags.push("sel");
    if (context?.clipboard_text) flags.push("clip");
    if (context?.window_title) flags.push("title");
    if (context?.screenshot_path) flags.push("shot");
    return flags;
  })();

  const handleKeyDown = async (e) => {
    // Cmd+Shift+D toggles a small debug panel showing captured context.
    if (e.key?.toLowerCase?.() === "d" && e.metaKey && e.shiftKey) {
      e.preventDefault();
      setShowDebug((v) => !v);
      return;
    }

    if (e.key !== "Enter") return;
    if (!query.trim() || loading) return;

    e.preventDefault();
    setLoading(true);

    try {
      // Prefer the "brain" layer (context + intent + prompt assembly).
      let result;
      try {
        const resp = await invoke("brain_run", {
          request: { input: query.trim(), debug: showDebug },
        });
        setLastEngine("brain_run");
        setLastInvokeError(null);
        setBrainDebug(resp?.debug ?? null);
        if (resp?.debug) console.log("[Capsule] brain debug:", resp.debug);
        result = resp?.text ?? "";
        if (!result) throw new Error("brain_run returned empty text");
      } catch (e) {
        console.warn("[Capsule] brain_run failed, falling back to llm_prompt/query_memory:", e);
        setLastEngine("brain_run_failed");
        setLastInvokeError(String(e?.message ?? e));
        setBrainDebug(null);
        try {
          result = await invoke("llm_prompt", { prompt: query.trim() });
          setLastEngine("llm_prompt");
          setLastInvokeError(null);
        } catch (e2) {
          console.warn("[Capsule] llm_prompt failed, falling back to query_memory:", e2);
          setLastEngine("query_memory");
          setLastInvokeError(String(e2?.message ?? e2));
          result = await invoke("query_memory", { query: query.trim() });
        }
      }

      // Debug mode: keep the capsule open so you can inspect context/debug output.
      if (showDebug) {
        setLastResult(result);
        return;
      }

      // Hide the capsule first so focus can return to the interrupted app.
      await onDismiss();

      // Hand the result back to Rust so it can paste it into the active app.
      await emit("inject_text", { text: result });
    } catch (err) {
      console.error("[Capsule] query_memory / inject_text error:", err);
    } finally {
      setLoading(false);
      setQuery("");
    }
  };

  return (
    /*
      Outer wrapper — full viewport, transparent, click-through on empty space.
      The pill itself is centered and ~580 px wide, Spotlight-style.
    */
    <div
      className="w-screen h-screen flex justify-center"
      onMouseDown={(e) => {
        // Clicking outside the pill dismisses the capsule.
        if (e.target === e.currentTarget) onDismiss();
      }}
    >
      <div
        className={[
          // ── shape ──────────────────────────────────────────────────────────
          "flex items-center gap-3",
          "w-[580px] px-5 py-3 rounded-2xl",
          // ── frosted glass ──────────────────────────────────────────────────
          "bg-black/60 backdrop-blur-2xl backdrop-saturate-150",
          // ── border + shadow ────────────────────────────────────────────────
          "border border-white/25",
          "shadow-[0_8px_32px_rgba(0,0,0,0.55)]",
          // ── text ───────────────────────────────────────────────────────────
          "text-white",
          // ── transition ─────────────────────────────────────────────────────
          "transition-all duration-150",
        ].join(" ")}
      >
        {/* Leading icon — magnifier, swapped for a spinner while loading */}
        <span className="text-white/50 text-lg select-none shrink-0">
          {loading ? (
            <svg
              className="animate-spin h-5 w-5 text-white/60"
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
              className="h-5 w-5 text-white/50"
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

        {/* Main text input */}
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={handleKeyDown}
          disabled={loading}
          placeholder={
            context?.app_name ? `Ask anything… (${context.app_name})` : "Ask anything…"
          }
          className={[
            "flex-1 bg-transparent outline-none border-none",
            "text-[15px] font-medium tracking-[-0.01em]",
            "text-white/95 placeholder-white/50 caret-white",
            "disabled:opacity-50",
          ].join(" ")}
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
        />

        {/* Trailing hint */}
        {!loading && (
          <span className="text-[11px] text-white/30 shrink-0 select-none">
            ↵ run
            {context?.app_name ? ` · ${context.app_name}` : ""}
            {ctxFlags.length ? ` · ${ctxFlags.join("·")}` : ""}
          </span>
        )}
      </div>

      {showDebug && (
        <pre
          className={[
            "mt-3 w-[580px] max-h-[260px] overflow-auto",
            "rounded-xl border border-white/15",
            "bg-black/55 backdrop-blur-2xl",
            "px-4 py-3 text-[11px] leading-relaxed",
            "text-white/80",
            "select-text",
          ].join(" ")}
        >
          {JSON.stringify(
            {
              show_capsule_context: context ?? null,
              last_brain_debug: brainDebug ?? null,
              last_result_preview: lastResult ?? null,
              last_engine: lastEngine ?? null,
              last_invoke_error: lastInvokeError ?? null,
            },
            null,
            2
          )}
        </pre>
      )}
    </div>
  );
}
