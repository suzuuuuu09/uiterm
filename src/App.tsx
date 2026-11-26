import { Tabs } from "@base-ui/react/tabs";
import { Globe, Plus, RotateCcw, Terminal as TerminalIcon, X } from "lucide-react";
import { useCallback, useEffect, useRef } from "react";
import { appRuntime } from "./app/runtime.ts";
import { t } from "./i18n/index.ts";
import { PocDiagnosticsDrawer } from "./poc/PocDiagnosticsDrawer.tsx";
import { pocStore, usePocStore } from "./poc/pocStore.ts";
import { appStore, useAppStore } from "./state/appStore.ts";
import { isMacPlatform, matchAppShortcut } from "./terminal/keyboard.ts";
import { TerminalViewport } from "./ui/TerminalViewport.tsx";

function isEditableElement(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.closest(".wterm-host")) return false;
  const tag = target.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target.isContentEditable;
}

/**
 * Minimal product tab strip for Phase 2.
 * Full workspace/tab/pane management is deferred to Phase 3.
 */
function TerminalTopBar({ onNewTerminal }: { readonly onNewTerminal: () => void }) {
  const locale = useAppStore((s) => s.settings.locale);
  const tabs = useAppStore((s) => s.tabs);
  const panes = useAppStore((s) => s.panes);
  const sessions = useAppStore((s) => s.sessions);
  const activeWorkspace = useAppStore((s) =>
    s.selection.activeWorkspaceId ? s.workspaces[s.selection.activeWorkspaceId] ?? null : null,
  );
  const activeTabId = useAppStore((s) => s.selection.activeTabId);
  const benchRunning = usePocStore((s) => s.benchRunning);

  const tabList = (activeWorkspace?.tabIds ?? [])
    .map((id) => tabs[id])
    .filter((tab): tab is NonNullable<typeof tab> => Boolean(tab));

  return (
    <header className="flex h-8 shrink-0 items-center justify-between border-b border-zinc-800/80 bg-zinc-900/90 px-2 select-none">
      <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
        <Tabs.Root
          value={activeTabId ?? ""}
          onValueChange={(value) => {
            if (typeof value === "string" && value && !benchRunning) {
              void appRuntime.actions.execute("tab.select", { tabId: value }, { caller: "gui" });
            }
          }}
          className="flex min-w-0 items-center"
        >
          <Tabs.List className="flex items-center gap-1">
            {tabList.map((tab) => {
              const pane = tab.activePaneId ? panes[tab.activePaneId] : undefined;
              const session = pane?.sessionId ? sessions[pane.sessionId] : undefined;
              const displayTitle = session?.title?.trim() || tab.title;
              return (
                <div key={tab.id} className="flex items-center">
                  <Tabs.Tab
                    value={tab.id}
                    disabled={benchRunning}
                    className="flex max-w-44 items-center gap-1.5 truncate rounded-l px-2 py-0.5 text-[11px] text-zinc-400 hover:bg-zinc-800/80 hover:text-zinc-200 disabled:opacity-50 data-[selected]:bg-zinc-800 data-[selected]:text-zinc-100"
                  >
                    <TerminalIcon className="h-3 w-3 shrink-0 text-emerald-400" />
                    <span className="truncate">{displayTitle}</span>
                  </Tabs.Tab>
                  <button
                    type="button"
                    disabled={benchRunning}
                    aria-label={`${t(locale, "app.closeTerminal")}: ${displayTitle}`}
                    onClick={() => {
                      void appRuntime.actions.execute(
                        "terminal.close",
                        { tabId: tab.id },
                        { caller: "gui" },
                      );
                    }}
                    className="rounded-r px-1 py-0.5 text-zinc-500 hover:bg-zinc-800 hover:text-zinc-200 disabled:opacity-50"
                  >
                    <X className="h-3 w-3" />
                  </button>
                </div>
              );
            })}
          </Tabs.List>
        </Tabs.Root>

        <button
          type="button"
          disabled={benchRunning}
          aria-label={t(locale, "app.newTerminal")}
          title={t(locale, "app.newTerminal")}
          onClick={onNewTerminal}
          className="rounded p-1 text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100 disabled:opacity-50"
        >
          <Plus className="h-3.5 w-3.5" />
        </button>
      </div>

      <div className="ml-2 flex shrink-0 items-center gap-1">
        <button
          type="button"
          onClick={() => {
            void appRuntime.actions.execute(
              "app.setLocale",
              { locale: locale === "ja" ? "en" : "ja" },
              { caller: "gui" },
            );
          }}
          className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100"
        >
          <Globe className="h-3 w-3" />
          {t(locale, "app.localeToggle")}
        </button>
      </div>
    </header>
  );
}

/**
 * Displays a compact recovery strip only when the active terminal's shell has
 * exited (`finished`) or its session connection was lost (`lost`).
 */
function SessionStatusBanner() {
  const locale = useAppStore((s) => s.settings.locale);
  const activeTabId = useAppStore((s) => s.selection.activeTabId);
  const activePane = useAppStore((s) =>
    s.selection.activePaneId ? s.panes[s.selection.activePaneId] ?? null : null,
  );
  const activeSession = useAppStore((s) =>
    activePane?.sessionId ? s.sessions[activePane.sessionId] ?? null : null,
  );

  const disconnected = useAppStore((s) => s.coreConnection.status === "disconnected");
  if (!activeTabId || !activePane?.sessionId) return null;
  if (activeSession?.state === "live" && !disconnected) return null;

  const message =
    activeSession?.state === "finished"
      ? activeSession.signal
        ? t(locale, "app.sessionSignaled", { signal: activeSession.signal })
        : t(locale, "app.sessionExited", { exitCode: activeSession.exitCode ?? "-" })
      : t(locale, "app.sessionLost");

  return (
    <div
      data-testid="session-status-banner"
      className="flex items-center justify-between border-t border-zinc-800 bg-zinc-900/95 px-3 py-1.5 text-xs text-zinc-300"
    >
      <span>{message}</span>
      <button
        type="button"
        onClick={() => {
          void appRuntime.actions.execute(
            "terminal.restart",
            { tabId: activeTabId },
            { caller: "gui" },
          );
        }}
        className="flex items-center gap-1 rounded bg-zinc-800 px-2 py-0.5 text-[11px] text-zinc-100 hover:bg-zinc-700"
      >
        <RotateCcw className="h-3 w-3" />
        {t(locale, "app.restartTerminal")}
      </button>
    </div>
  );
}

export function App() {
  const locale = useAppStore((s) => s.settings.locale);
  const tabCount = useAppStore((s) => Object.keys(s.tabs).length);
  const diagnosticsPanelVisible = useAppStore((s) => s.ui.diagnosticsPanelVisible);

  const initializedRef = useRef(false);
  const tabGenerationRef = useRef(0);

  // Support opt-in `?poc=1` query flag for development/benchmarking without showing PoC UI by default.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const params = new URLSearchParams(window.location.search);
    if (params.get("poc") === "1") {
      appStore.getState().setDiagnosticsPanelVisible(true);
    }
  }, []);

  const spawnTabs = useCallback(async (count: number, replace = false) => {
    const host = appRuntime.getMountRoot();
    if (!host) return;

    if (replace) {
      tabGenerationRef.current++;
      appRuntime.sessionManager.disposeAll();
      appStore.getState().clearWorkspaceTabs();
      pocStore.getState().resetStats();
    }

    const generation = tabGenerationRef.current;

    for (let i = 0; i < count; i++) {
      if (generation !== tabGenerationRef.current) return;
      const activate = replace ? i === 0 : i === count - 1;
      const result = await appRuntime.actions.execute(
        "terminal.create",
        { activate },
        { caller: "gui" },
      );
      if (!result.ok) {
        break;
      }
    }
  }, []);

  const handleHostReady = useCallback(
    (el: HTMLDivElement | null) => {
      appRuntime.setMountRoot(el);
      if (!el) {
        // React StrictMode reattaches the same host before the microtask runs.
        queueMicrotask(() => {
          if (!appRuntime.getMountRoot()) {
            tabGenerationRef.current++;
            initializedRef.current = false;
            appRuntime.sessionManager.stopEventStream();
            appRuntime.sessionManager.disposeAll();
            appStore.getState().clearWorkspaceTabs();
          }
        });
      } else if (!initializedRef.current) {
        initializedRef.current = true;
        void spawnTabs(1, true);
      }
    },
    [spawnTabs],
  );

  const handleCloseAll = useCallback(() => {
    tabGenerationRef.current++;
    appRuntime.sessionManager.disposeAll();
    appStore.getState().clearWorkspaceTabs();
    pocStore.getState().resetStats();
  }, []);

  // Global keyboard shortcuts wired to ActionRegistry, strictly guarded against IME composition.
  useEffect(() => {
    const isMac = isMacPlatform();
    const onKeyDown = (event: KeyboardEvent) => {
      const state = appStore.getState();
      const activePaneId = state.selection.activePaneId;
      const isEditableTarget = isEditableElement(event.target);
      const hasSelection = activePaneId && !isEditableTarget
        ? appRuntime.sessionManager.hasSelection(activePaneId)
        : false;
      const match = matchAppShortcut(event, {
        isMac,
        isComposing: state.ui.isComposing,
        hasSelection,
        isEditableTarget,
      });
      if (!match) return;
      event.preventDefault();
      event.stopPropagation();
      void appRuntime.actions.execute(
        match.actionId,
        match.args as never,
        { caller: "keybind" },
      );
    };

    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, []);

  // Synchronize OS window focus/blur with active terminal focus events (for Neovim/fish).
  useEffect(() => {
    const onFocus = () => {
      appRuntime.sessionManager.setWindowFocused(true);
    };
    const onBlur = () => {
      appRuntime.sessionManager.setWindowFocused(false);
    };
    appRuntime.sessionManager.setWindowFocused(document.hasFocus());
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  return (
    <div className="flex h-screen w-screen flex-col bg-zinc-950 text-xs text-zinc-100">
      <TerminalTopBar onNewTerminal={() => void spawnTabs(1, false)} />

      <div className="flex min-h-0 flex-1">
        <div className="relative flex min-w-0 flex-1 flex-col">
          <TerminalViewport onHostReady={handleHostReady} />
          {tabCount === 0 ? (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-zinc-500">
              <span>{t(locale, "app.emptyTerminalHint")}</span>
            </div>
          ) : null}
          <SessionStatusBanner />
        </div>

        {diagnosticsPanelVisible ? (
          <PocDiagnosticsDrawer onSpawnTabs={spawnTabs} onCloseAll={handleCloseAll} />
        ) : null}
      </div>
    </div>
  );
}
