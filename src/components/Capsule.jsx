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
export default function Capsule({ onDismiss }) {
  const inputRef = useRef(null);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);

  // Auto-focus every time the capsule mounts (i.e. every time it becomes visible).
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const handleKeyDown = async (e) => {
    if (e.key !== "Enter") return;
    if (!query.trim() || loading) return;

    e.preventDefault();
    setLoading(true);

    try {
      // Invoke the Rust `query_memory` command with the user's prompt.
      // Returns the stub string for now; will return real AI output later.
      const result = await invoke("query_memory", { query: query.trim() });

      // Hand the result back to Rust so it can paste it into the active app.
      await emit("inject_text", { text: result });
    } catch (err) {
      console.error("[Capsule] query_memory / inject_text error:", err);
    } finally {
      setLoading(false);
      setQuery("");
      onDismiss();
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
          placeholder="Ask anything…"
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
          </span>
        )}
      </div>
    </div>
  );
}
