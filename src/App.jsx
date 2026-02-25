import { useEffect, useState, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import Capsule from "./components/Capsule.jsx";

export default function App() {
  const [visible, setVisible] = useState(false);

  // ── show-capsule (emitted by Rust interceptor on `//`) ────────────────────
  useEffect(() => {
    const unlisten = listen("show-capsule", async () => {
      // Window is shown from Rust via NSWindow APIs to stay on
      // the current Space (including fullscreen).  We only flip state.
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

  return <Capsule onDismiss={hide} />;
}
