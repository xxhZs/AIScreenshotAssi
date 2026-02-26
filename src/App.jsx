import { useEffect, useState, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import Capsule from "./components/Capsule.jsx";

export default function App() {
  const [visible, setVisible] = useState(false);
  const [context, setContext] = useState(null);

  // ── show-capsule (emitted by Rust interceptor on `//`) ────────────────────
  useEffect(() => {
    const unlisten = listen("show-capsule", async (event) => {
      // Window is shown from Rust via NSWindow APIs to stay on
      // the current Space (including fullscreen).  We only flip state.
      try {
        const payload = event?.payload ?? null;
        if (!payload) {
          setContext(null);
        } else if (typeof payload === "string") {
          setContext(JSON.parse(payload));
        } else {
          setContext(payload);
        }
      } catch {
        setContext(null);
      }
      setVisible(true);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // ── Escape → hide capsule ─────────────────────────────────────────────────
  const hide = useCallback(async () => {
    setVisible(false);
    const win = getCurrentWindow();
    await win.hide();
  }, []);

  useEffect(() => {
    const onKeyDown = (e) => {
      if (e.key === "Escape") hide();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [hide]);

  if (!visible) return null;

  return <Capsule onDismiss={hide} context={context} />;
}
