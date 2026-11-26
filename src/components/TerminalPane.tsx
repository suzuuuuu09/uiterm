import { useEffect, useRef } from "react";
import { pocStore, usePocStore } from "../store/pocStore.ts";
import { sessionManager } from "../terminal/sessionManager.ts";

interface TerminalViewportProps {
  onHostReady: (host: HTMLDivElement | null) => void;
}

/**
 * Hosts all mounted terminal DOM subtrees.
 * Observes IME composition and keydown events in capture phase for Phase 0 diagnostics
 * without intercepting or mutating terminal behavior.
 */
export function TerminalViewport({ onHostReady }: TerminalViewportProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const activeTabId = usePocStore((s) => s.activeTabId);

  useEffect(() => {
    const el = hostRef.current;
    onHostReady(el);
    if (!el) return;

    const onCompStart = (e: CompositionEvent) => {
      pocStore.getState().setIsComposing(true);
      pocStore.getState().pushLog("ime", `compositionstart data="${e.data}"`);
    };
    const onCompUpdate = (e: CompositionEvent) => {
      pocStore.getState().pushLog("ime", `compositionupdate data="${e.data}"`);
    };
    const onCompEnd = (e: CompositionEvent) => {
      pocStore.getState().setIsComposing(false);
      pocStore.getState().pushLog("ime", `compositionend data="${e.data}"`);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.isComposing || e.keyCode === 229 || e.key === "Enter" || e.key === "Process") {
        pocStore
          .getState()
          .pushLog(
            "ime",
            `keydown key="${e.key}" code="${e.code}" isComposing=${e.isComposing} keyCode=${e.keyCode}`,
          );
      }
    };

    el.addEventListener("compositionstart", onCompStart, true);
    el.addEventListener("compositionupdate", onCompUpdate, true);
    el.addEventListener("compositionend", onCompEnd, true);
    el.addEventListener("keydown", onKeyDown, true);

    return () => {
      el.removeEventListener("compositionstart", onCompStart, true);
      el.removeEventListener("compositionupdate", onCompUpdate, true);
      el.removeEventListener("compositionend", onCompEnd, true);
      el.removeEventListener("keydown", onKeyDown, true);
      onHostReady(null);
    };
  }, [onHostReady]);

  useEffect(() => {
    sessionManager.setActive(activeTabId);
  }, [activeTabId]);

  return (
    <div
      ref={hostRef}
      data-testid="terminal-viewport"
      className="relative flex-1 overflow-hidden bg-zinc-950"
    />
  );
}
