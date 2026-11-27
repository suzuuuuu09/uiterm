import { useEffect, useRef } from "react";
import { appRuntime } from "../app/runtime.ts";
import { pocStore } from "../poc/pocStore.ts";
import { appStore, useAppStore } from "../state/appStore.ts";

interface TerminalViewportProps {
  onHostReady: (host: HTMLDivElement | null) => void;
}

/**
 * Product terminal viewport hosting mounted terminal DOM subtrees.
 *
 * Privacy invariant:
 * - Tracks `ui.isComposing` state for keybinding protection during Japanese IME composition.
 * - NEVER logs IME composition/committed text (`e.data`) or typed characters.
 * - When PoC diagnostics are explicitly enabled, logs only structural event metadata
 *   (event name, keyCode, isComposing flag).
 */
export function TerminalViewport({ onHostReady }: TerminalViewportProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const activePaneId = useAppStore((s) => s.selection.activePaneId);

  useEffect(() => {
    const el = hostRef.current;
    onHostReady(el);
    if (!el) return;

    const onCompStart = () => {
      appStore.getState().setIsComposing(true);
      if (pocStore.getState().diagnosticsEnabled) {
        pocStore.getState().pushLog("ime", "compositionstart");
      }
    };
    const onCompUpdate = () => {
      if (pocStore.getState().diagnosticsEnabled) {
        pocStore.getState().pushLog("ime", "compositionupdate");
      }
    };
    const onCompEnd = () => {
      appStore.getState().setIsComposing(false);
      if (pocStore.getState().diagnosticsEnabled) {
        pocStore.getState().pushLog("ime", "compositionend");
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.isComposing || e.keyCode === 229 || e.key === "Enter" || e.key === "Process") {
        if (pocStore.getState().diagnosticsEnabled) {
          pocStore
            .getState()
            .pushLog("ime", `keydown isComposing=${e.isComposing} keyCode=${e.keyCode}`);
        }
      }
    };

    const onBlur = (e: FocusEvent) => {
      const doc = el.ownerDocument;
      if (e.relatedTarget === null && !doc.hasFocus() && doc.activeElement === e.target) return;
      appStore.getState().setIsComposing(false);
    };

    el.addEventListener("blur", onBlur, true);
    el.addEventListener("compositionstart", onCompStart, true);
    el.addEventListener("compositionupdate", onCompUpdate, true);
    el.addEventListener("compositionend", onCompEnd, true);
    el.addEventListener("keydown", onKeyDown, true);

    return () => {
      el.removeEventListener("blur", onBlur, true);
      el.removeEventListener("compositionstart", onCompStart, true);
      el.removeEventListener("compositionupdate", onCompUpdate, true);
      el.removeEventListener("compositionend", onCompEnd, true);
      el.removeEventListener("keydown", onKeyDown, true);
      appStore.getState().setIsComposing(false);
      onHostReady(null);
    };
  }, [onHostReady]);

  useEffect(() => {
    if (activePaneId === null || appRuntime.sessionManager.get(activePaneId)) {
      appRuntime.sessionManager.setActive(activePaneId);
    }
  }, [activePaneId]);

  return (
    <div
      ref={hostRef}
      data-testid="terminal-viewport"
      className="relative flex-1 overflow-hidden bg-zinc-950"
    />
  );
}
