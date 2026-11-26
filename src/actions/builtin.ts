import * as v from "valibot";
import type { StoreApi } from "zustand/vanilla";
import { AppError } from "../errors/appError.ts";
import type { AppState } from "../state/appStore.ts";
import {
  clampFontSize,
  clampLineHeight,
  DEFAULT_FONT_SIZE,
  FONT_SIZE_STEP,
  MAX_FONT_SIZE,
  MAX_LINE_HEIGHT,
  MIN_FONT_SIZE,
  MIN_LINE_HEIGHT,
} from "../terminal/fonts.ts";
import type { TerminalSessionManager } from "../terminal/sessionManager.ts";
import { TerminalColsSchema, TerminalRowsSchema } from "../validation/ipcSchemas.ts";
import {
  CursorBlinkModeSchema,
  CursorStyleSchema,
  LocaleSchema,
  TerminalBackendIdSchema,
} from "../validation/settingsSchemas.ts";
import type { ActionRegistry } from "./registry.ts";

export interface BuiltinActionDeps {
  readonly store: StoreApi<AppState>;
  readonly sessionManager: TerminalSessionManager;
  readonly getMountRoot: () => HTMLElement | null;
  readonly writeClipboard?: (text: string) => Promise<void>;
  readonly readClipboard?: () => Promise<string>;
  readonly isBusy?: () => boolean;
}

const EmptyArgsSchema = v.object({});

const OptionalTabIdArgsSchema = v.object({
  tabId: v.optional(v.pipe(v.string(), v.minLength(1))),
});

const TerminalPasteArgsSchema = v.object({
  tabId: v.optional(v.pipe(v.string(), v.minLength(1))),
  text: v.optional(v.string()),
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

const SetFontArgsSchema = v.object({
  fontFamily: v.optional(v.pipe(v.string(), v.minLength(1), v.maxLength(512))),
  fontSize: v.optional(v.pipe(v.number(), v.minValue(MIN_FONT_SIZE), v.maxValue(MAX_FONT_SIZE))),
  lineHeight: v.optional(
    v.pipe(v.number(), v.minValue(MIN_LINE_HEIGHT), v.maxValue(MAX_LINE_HEIGHT)),
  ),
  nerdFontFallback: v.optional(v.boolean()),
});

const SetCursorArgsSchema = v.object({
  cursorStyle: v.optional(CursorStyleSchema),
  cursorBlink: v.optional(CursorBlinkModeSchema),
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

  const hasOwnTab = (tabId: string): boolean => Object.hasOwn(deps.store.getState().tabs, tabId);

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
        while (
          Object.hasOwn(state.tabs, tabId) ||
          Object.hasOwn(state.panes, tabId) ||
          deps.sessionManager.get(tabId)
        ) {
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
          const selectedPaneId = deps.store.getState().selection.activePaneId;
          if (selectedPaneId === tabId) {
            deps.sessionManager.setActive(tabId);
          } else if (selectedPaneId === null || deps.sessionManager.get(selectedPaneId)) {
            deps.sessionManager.setActive(selectedPaneId);
          } else {
            deps.sessionManager.hideSession(tabId);
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
        if (!hasOwnTab(targetId)) {
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
        if (!hasOwnTab(targetId)) {
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
        if (!hasOwnTab(targetId)) {
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
        if (!hasOwnTab(targetId)) {
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
      id: "terminal.paste",
      owner: "terminal",
      titleKey: "action.terminal.paste.title",
      descriptionKey: "action.terminal.paste.description",
      argsSchema: TerminalPasteArgsSchema,
      canExecute: (args) => {
        const targetId = resolveTabId(args?.tabId);
        if (!targetId) {
          return { allowed: false, reasonKey: "action.unavailable.noActiveTab" };
        }
        if (!hasOwnTab(targetId)) {
          return { allowed: false, reasonKey: "action.unavailable.tabNotFound" };
        }
        return { allowed: true };
      },
      execute: async (args) => {
        const targetId = resolveTabId(args.tabId)!;
        const paneId = deps.store.getState().tabs[targetId]?.activePaneId ?? targetId;
        const session = deps.sessionManager.get(paneId);
        if (!session) return { pastedLength: 0 };
        let text = args.text;
        if (text === undefined) {
          if (deps.readClipboard) {
            text = await deps.readClipboard();
          } else if (typeof navigator !== "undefined" && navigator.clipboard?.readText) {
            text = await navigator.clipboard.readText();
          } else {
            throw new AppError({
              code: "ACTION_EXECUTION_FAILED",
              messageKey: "error.action.failed",
              messageParams: { actionId: "terminal.paste" },
              devMessage: "Clipboard read API is unavailable",
            });
          }
        }
        if (deps.sessionManager.get(paneId) !== session) return { pastedLength: 0 };
        const pasted = deps.sessionManager.pasteText(paneId, text);
        return { pastedLength: pasted ? text.length : 0 };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.selectAll",
      owner: "terminal",
      titleKey: "action.terminal.selectAll.title",
      descriptionKey: "action.terminal.selectAll.description",
      argsSchema: OptionalTabIdArgsSchema,
      canExecute: (args) => {
        const targetId = resolveTabId(args?.tabId);
        if (!targetId) {
          return { allowed: false, reasonKey: "action.unavailable.noActiveTab" };
        }
        if (!hasOwnTab(targetId)) {
          return { allowed: false, reasonKey: "action.unavailable.tabNotFound" };
        }
        return { allowed: true };
      },
      execute: async (args) => {
        const targetId = resolveTabId(args.tabId)!;
        const paneId = deps.store.getState().tabs[targetId]?.activePaneId ?? targetId;
        const selected = await deps.sessionManager.selectAll(paneId);
        return { selected };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.clearSelection",
      owner: "terminal",
      titleKey: "action.terminal.clearSelection.title",
      descriptionKey: "action.terminal.clearSelection.description",
      argsSchema: OptionalTabIdArgsSchema,
      canExecute: (args) => {
        const targetId = resolveTabId(args?.tabId);
        if (!targetId) {
          return { allowed: false, reasonKey: "action.unavailable.noActiveTab" };
        }
        if (!hasOwnTab(targetId)) {
          return { allowed: false, reasonKey: "action.unavailable.tabNotFound" };
        }
        return { allowed: true };
      },
      execute: (args) => {
        const targetId = resolveTabId(args.tabId)!;
        const paneId = deps.store.getState().tabs[targetId]?.activePaneId ?? targetId;
        const cleared = deps.sessionManager.clearSelection(paneId);
        return { cleared };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.restart",
      owner: "terminal",
      titleKey: "action.terminal.restart.title",
      descriptionKey: "action.terminal.restart.description",
      argsSchema: OptionalTabIdArgsSchema,
      canExecute: (args) => {
        if (deps.isBusy?.()) {
          return { allowed: false, reasonKey: "action.unavailable.busy" };
        }
        if (!deps.getMountRoot()) {
          return { allowed: false, reasonKey: "action.unavailable.noHost" };
        }
        const targetId = resolveTabId(args?.tabId);
        if (!targetId) {
          return { allowed: false, reasonKey: "action.unavailable.noActiveTab" };
        }
        if (!hasOwnTab(targetId)) {
          return { allowed: false, reasonKey: "action.unavailable.tabNotFound" };
        }
        const paneId = deps.store.getState().tabs[targetId]?.activePaneId ?? targetId;
        if (deps.sessionManager.isCreating(paneId)) {
          return { allowed: false, reasonKey: "action.unavailable.busy" };
        }
        return { allowed: true };
      },
      execute: async (args) => {
        const host = deps.getMountRoot();
        if (!host) {
          throw new Error("Terminal viewport host is not mounted");
        }
        const targetId = resolveTabId(args.tabId)!;
        const state = deps.store.getState();
        const paneId = state.tabs[targetId]?.activePaneId ?? targetId;
        const pane = state.panes[paneId];
        const isActive = state.selection.activePaneId === paneId;

        deps.sessionManager.disposeSession(paneId);

        const session = await deps.sessionManager.createSession(
          paneId,
          state.settings.renderer,
          host,
          !isActive,
          {
            cols: pane?.cols,
            rows: pane?.rows,
          },
        );
        // Reconcile visibility and focus with the current active pane in case
        // the user switched tabs while the restarted session was being created.
        deps.sessionManager.setActive(deps.store.getState().selection.activePaneId);
        return { tabId: targetId, paneId, sessionId: session.sessionId };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.zoomIn",
      owner: "app",
      titleKey: "action.terminal.zoomIn.title",
      descriptionKey: "action.terminal.zoomIn.description",
      argsSchema: EmptyArgsSchema,
      execute: () => {
        const nextSize = clampFontSize(deps.store.getState().settings.fontSize + FONT_SIZE_STEP);
        deps.store.getState().updateSettings({ fontSize: nextSize });
        deps.sessionManager.applySettingsToAll(deps.store.getState().settings);
        return { fontSize: nextSize };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.zoomOut",
      owner: "app",
      titleKey: "action.terminal.zoomOut.title",
      descriptionKey: "action.terminal.zoomOut.description",
      argsSchema: EmptyArgsSchema,
      execute: () => {
        const nextSize = clampFontSize(deps.store.getState().settings.fontSize - FONT_SIZE_STEP);
        deps.store.getState().updateSettings({ fontSize: nextSize });
        deps.sessionManager.applySettingsToAll(deps.store.getState().settings);
        return { fontSize: nextSize };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "terminal.resetFontSize",
      owner: "app",
      titleKey: "action.terminal.resetFontSize.title",
      descriptionKey: "action.terminal.resetFontSize.description",
      argsSchema: EmptyArgsSchema,
      execute: () => {
        deps.store.getState().updateSettings({ fontSize: DEFAULT_FONT_SIZE });
        deps.sessionManager.applySettingsToAll(deps.store.getState().settings);
        return { fontSize: DEFAULT_FONT_SIZE };
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
        if (!args || !hasOwnTab(args.tabId)) {
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
      id: "app.setFont",
      owner: "app",
      titleKey: "action.app.setFont.title",
      descriptionKey: "action.app.setFont.description",
      argsSchema: SetFontArgsSchema,
      execute: (args) => {
        const current = deps.store.getState().settings;
        const patch = {
          ...(args.fontFamily !== undefined ? { fontFamily: args.fontFamily } : {}),
          ...(args.fontSize !== undefined ? { fontSize: clampFontSize(args.fontSize) } : {}),
          ...(args.lineHeight !== undefined
            ? { lineHeight: clampLineHeight(args.lineHeight) }
            : {}),
          ...(args.nerdFontFallback !== undefined
            ? { nerdFontFallback: args.nerdFontFallback }
            : {}),
        };
        if (Object.keys(patch).length > 0) {
          deps.store.getState().updateSettings(patch);
          deps.sessionManager.applySettingsToAll(deps.store.getState().settings);
        }
        const next = deps.store.getState().settings;
        return {
          fontFamily: next.fontFamily,
          fontSize: next.fontSize,
          lineHeight: next.lineHeight,
          nerdFontFallback: next.nerdFontFallback ?? current.nerdFontFallback,
        };
      },
    }),
  );

  unregisters.push(
    registry.register({
      id: "app.setCursor",
      owner: "app",
      titleKey: "action.app.setCursor.title",
      descriptionKey: "action.app.setCursor.description",
      argsSchema: SetCursorArgsSchema,
      execute: (args) => {
        const patch = {
          ...(args.cursorStyle !== undefined ? { cursorStyle: args.cursorStyle } : {}),
          ...(args.cursorBlink !== undefined ? { cursorBlink: args.cursorBlink } : {}),
        };
        if (Object.keys(patch).length > 0) {
          deps.store.getState().updateSettings(patch);
          deps.sessionManager.applySettingsToAll(deps.store.getState().settings);
        }
        const next = deps.store.getState().settings;
        return {
          cursorStyle: next.cursorStyle,
          cursorBlink: next.cursorBlink,
        };
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

  unregisters.push(
    registry.register({
      id: "app.toggleDiagnostics",
      owner: "ui",
      titleKey: "action.app.toggleDiagnostics.title",
      descriptionKey: "action.app.toggleDiagnostics.description",
      argsSchema: EmptyArgsSchema,
      execute: () => {
        deps.store.getState().toggleDiagnosticsPanel();
        return {
          diagnosticsPanelVisible: deps.store.getState().ui.diagnosticsPanelVisible,
        };
      },
    }),
  );

  return () => {
    for (const unregister of unregisters) {
      unregister();
    }
  };
}
