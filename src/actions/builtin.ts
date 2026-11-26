import * as v from "valibot";
import type { StoreApi } from "zustand/vanilla";
import { AppError } from "../errors/appError.ts";
import type { AppState } from "../state/appStore.ts";
import type { TerminalSessionManager } from "../terminal/sessionManager.ts";
import { TerminalColsSchema, TerminalRowsSchema } from "../validation/ipcSchemas.ts";
import { LocaleSchema, TerminalBackendIdSchema } from "../validation/settingsSchemas.ts";
import type { ActionRegistry } from "./registry.ts";

export interface BuiltinActionDeps {
  readonly store: StoreApi<AppState>;
  readonly sessionManager: TerminalSessionManager;
  readonly getMountRoot: () => HTMLElement | null;
  readonly writeClipboard?: (text: string) => Promise<void>;
  readonly isBusy?: () => boolean;
}

const EmptyArgsSchema = v.object({});

const OptionalTabIdArgsSchema = v.object({
  tabId: v.optional(v.pipe(v.string(), v.minLength(1))),
});

const TerminalCreateArgsSchema = v.object({
  title: v.optional(v.pipe(v.string(), v.minLength(1), v.maxLength(128))),
  cwd: v.optional(v.pipe(v.string(), v.minLength(1))),
  cols: v.optional(TerminalColsSchema),
  rows: v.optional(TerminalRowsSchema),
  activate: v.optional(v.boolean()),
});

const TabSelectArgsSchema = v.object({
  tabId: v.pipe(v.string(), v.minLength(1)),
});

const SetLocaleArgsSchema = v.object({
  locale: LocaleSchema,
});

const SetRendererArgsSchema = v.object({
  renderer: TerminalBackendIdSchema,
});

/**
 * Registers the Phase 1 built-in actions onto an `ActionRegistry`.
 *
 * Ownership contract:
 * - `terminal.*` actions coordinate `TerminalSessionManager` (PTY/WASM) and `AppStore` (Tab/Pane metadata).
 * - `tab.*` actions mutate layout/selection state in `AppStore` and sync visibility/focus in `TerminalSessionManager`.
 * - `app.*` actions mutate UI flags or `UserSettings` in `AppStore`.
 */
export function registerBuiltinActions(
  registry: ActionRegistry,
  deps: BuiltinActionDeps,
): () => void {
  let nextTabSeq = 1;
  const unregisters: Array<() => void> = [];

  const resolveTabId = (explicitTabId?: string): string | null =>
    explicitTabId ?? deps.store.getState().selection.activeTabId;

  unregisters.push(
    registry.register({
      id: "terminal.create",
      owner: "terminal",
      titleKey: "action.terminal.create.title",
      descriptionKey: "action.terminal.create.description",
      argsSchema: TerminalCreateArgsSchema,
      canExecute: () => {
        if (deps.isBusy?.()) {
          return { allowed: false, reasonKey: "action.unavailable.busy" };
        }
        if (!deps.getMountRoot()) {
          return { allowed: false, reasonKey: "action.unavailable.noHost" };
        }
        return { allowed: true };
      },
      execute: async (args) => {
        const host = deps.getMountRoot();
        if (!host) {
          throw new Error("Terminal viewport host is not mounted");
        }
        const state = deps.store.getState();
        let tabId = `term-${nextTabSeq++}`;
        while (state.tabs[tabId] || state.panes[tabId] || deps.sessionManager.get(tabId)) {
          tabId = `term-${nextTabSeq++}`;
        }
        const title = args.title ?? tabId;
        const activate = args.activate ?? true;
        const renderer = state.settings.renderer;

        let addedTab = false;
        deps.store.getState().addTerminalTab({
          tabId,
          paneId: tabId,
          title,
          cols: args.cols ?? 80,
          rows: args.rows ?? 24,
          activate,
        });
        addedTab = true;

        try {
          const session = await deps.sessionManager.createSession(tabId, renderer, host, !activate, {
            cwd: args.cwd,
            cols: args.cols,
            rows: args.rows,
          });
          if (activate) {
            const selectedPaneId = deps.store.getState().selection.activePaneId;
            if (selectedPaneId === null || deps.sessionManager.get(selectedPaneId)) {
              deps.sessionManager.setActive(selectedPaneId);
              if (selectedPaneId === tabId) {
                deps.sessionManager.focusSession(tabId);
              }
            } else {
              session.container.style.display = "none";
            }
          }
          return {
            tabId,
            paneId: tabId,
            sessionId: session.sessionId,
          };
        } catch (err) {
          if (addedTab) {
            deps.store.getState().removeTab(tabId);
            if (activate) {
              deps.sessionManager.setActive(deps.store.getState().selection.activePaneId);
            }
          }
          throw err;
        }
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.close",
      owner: "terminal",
      titleKey: "action.terminal.close.title",
      descriptionKey: "action.terminal.close.description",
      argsSchema: OptionalTabIdArgsSchema,
      canExecute: (args) => {
        if (deps.isBusy?.()) {
          return { allowed: false, reasonKey: "action.unavailable.busy" };
        }
        const targetId = resolveTabId(args?.tabId);
        if (!targetId) {
          return { allowed: false, reasonKey: "action.unavailable.noActiveTab" };
        }
        if (!deps.store.getState().tabs[targetId]) {
          return { allowed: false, reasonKey: "action.unavailable.tabNotFound" };
        }
        return { allowed: true };
      },
      execute: (args) => {
        const targetId = resolveTabId(args.tabId)!;
        const tab = deps.store.getState().tabs[targetId];
        if (tab) {
          for (const paneId of tab.paneIds) {
            deps.sessionManager.disposeSession(paneId);
          }
        } else {
          deps.sessionManager.disposeSession(targetId);
        }
        deps.store.getState().removeTab(targetId);
        const nextActive = deps.store.getState().selection.activePaneId;
        deps.sessionManager.setActive(nextActive);
        return { closedTabId: targetId };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.focus",
      owner: "terminal",
      titleKey: "action.terminal.focus.title",
      descriptionKey: "action.terminal.focus.description",
      argsSchema: OptionalTabIdArgsSchema,
      canExecute: (args) => {
        const targetId = resolveTabId(args?.tabId);
        if (!targetId) {
          return { allowed: false, reasonKey: "action.unavailable.noActiveTab" };
        }
        if (!deps.store.getState().tabs[targetId]) {
          return { allowed: false, reasonKey: "action.unavailable.tabNotFound" };
        }
        return { allowed: true };
      },
      execute: (args) => {
        const targetId = resolveTabId(args.tabId)!;
        deps.store.getState().selectTab(targetId);
        const activePaneId = deps.store.getState().selection.activePaneId ?? targetId;
        deps.sessionManager.setActive(activePaneId);
        deps.sessionManager.focusSession(activePaneId);
        return { focusedTabId: targetId };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.resync",
      owner: "terminal",
      titleKey: "action.terminal.resync.title",
      descriptionKey: "action.terminal.resync.description",
      argsSchema: OptionalTabIdArgsSchema,
      canExecute: (args) => {
        const targetId = resolveTabId(args?.tabId);
        if (!targetId) {
          return { allowed: false, reasonKey: "action.unavailable.noActiveTab" };
        }
        if (!deps.store.getState().tabs[targetId]) {
          return { allowed: false, reasonKey: "action.unavailable.tabNotFound" };
        }
        return { allowed: true };
      },
      execute: async (args) => {
        const targetId = resolveTabId(args.tabId)!;
        const paneId = deps.store.getState().tabs[targetId]?.activePaneId ?? targetId;
        await deps.sessionManager.resyncSession(paneId);
        return { resyncedTabId: targetId };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.copySelection",
      owner: "terminal",
      titleKey: "action.terminal.copySelection.title",
      descriptionKey: "action.terminal.copySelection.description",
      argsSchema: OptionalTabIdArgsSchema,
      canExecute: (args) => {
        const targetId = resolveTabId(args?.tabId);
        if (!targetId) {
          return { allowed: false, reasonKey: "action.unavailable.noActiveTab" };
        }
        if (!deps.store.getState().tabs[targetId]) {
          return { allowed: false, reasonKey: "action.unavailable.tabNotFound" };
        }
        return { allowed: true };
      },
      execute: async (args) => {
        const targetId = resolveTabId(args.tabId)!;
        const paneId = deps.store.getState().tabs[targetId]?.activePaneId ?? targetId;
        const session = deps.sessionManager.get(paneId);
        const selection = (await session?.adapter.getSelection()) ?? "";
        if (selection.length > 0) {
          if (deps.writeClipboard) {
            await deps.writeClipboard(selection);
          } else if (typeof navigator !== "undefined" && navigator.clipboard) {
            await navigator.clipboard.writeText(selection);
          } else {
            throw new AppError({
              code: "ACTION_EXECUTION_FAILED",
              messageKey: "error.action.failed",
              messageParams: { actionId: "terminal.copySelection" },
              devMessage: "Clipboard API is unavailable",
            });
          }
        }
        return { copiedLength: selection.length };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "tab.select",
      owner: "state",
      titleKey: "action.tab.select.title",
      descriptionKey: "action.tab.select.description",
      argsSchema: TabSelectArgsSchema,
      canExecute: (args) => {
        if (deps.isBusy?.()) {
          return { allowed: false, reasonKey: "action.unavailable.busy" };
        }
        if (!args || !deps.store.getState().tabs[args.tabId]) {
          return { allowed: false, reasonKey: "action.unavailable.tabNotFound" };
        }
        return { allowed: true };
      },
      execute: (args) => {
        deps.store.getState().selectTab(args.tabId);
        const paneId = deps.store.getState().selection.activePaneId;
        deps.sessionManager.setActive(paneId);
        return { activeTabId: args.tabId };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "app.setLocale",
      owner: "app",
      titleKey: "action.app.setLocale.title",
      descriptionKey: "action.app.setLocale.description",
      argsSchema: SetLocaleArgsSchema,
      execute: (args) => {
        deps.store.getState().updateSettings({ locale: args.locale });
        return { locale: args.locale };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "app.setRenderer",
      owner: "app",
      titleKey: "action.app.setRenderer.title",
      descriptionKey: "action.app.setRenderer.description",
      argsSchema: SetRendererArgsSchema,
      canExecute: () => {
        if (deps.isBusy?.()) {
          return { allowed: false, reasonKey: "action.unavailable.busy" };
        }
        return { allowed: true };
      },
      execute: (args) => {
        deps.store.getState().updateSettings({ renderer: args.renderer });
        return { renderer: args.renderer };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "app.toggleSidebar",
      owner: "ui",
      titleKey: "action.app.toggleSidebar.title",
      descriptionKey: "action.app.toggleSidebar.description",
      argsSchema: EmptyArgsSchema,
      execute: () => {
        deps.store.getState().toggleSidebar();
        return { sidebarVisible: deps.store.getState().ui.sidebarVisible };
      },
    }),
  );

  return () => {
    for (const unregister of unregisters) {
      unregister();
    }
  };
}
