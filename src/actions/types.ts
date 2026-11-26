import type * as v from "valibot";
import type { AppError } from "../errors/appError.ts";
import type { TranslationKey, TranslationParams } from "../i18n/index.ts";
import type { SessionId } from "../ipc/types.ts";
import type { PaneId, TabId } from "../state/types.ts";
import type { TerminalBackendId } from "../validation/settingsSchemas.ts";
import type { Locale } from "../i18n/types.ts";

/**
 * Who invoked the action (GUI button/menu, CLI command, keybinding, TS config, event hook, internal).
 */
export type ActionCaller = "gui" | "cli" | "keybind" | "config" | "hook" | "internal";

/**
 * Which subsystem owns the mutated state or side effect of the action.
 */
export type ActionOwner = "ui" | "state" | "terminal" | "app";

export interface ActionContext {
  readonly caller: ActionCaller;
  readonly timestamp: number;
  readonly signal?: AbortSignal;
}

export type ActionAvailability =
  | { readonly allowed: true }
  | {
      readonly allowed: false;
      readonly reasonKey: TranslationKey;
      readonly reasonParams?: TranslationParams;
    };

export type ActionResult<TResult> =
  | { readonly ok: true; readonly value: TResult }
  | { readonly ok: false; readonly error: AppError };

/**
 * Canonical catalog of built-in typed actions (Phase 1).
 * Each entry defines its argument type and return type.
 */
export interface BuiltinActionMap {
  "terminal.create": {
    args: {
      title?: string;
      cwd?: string;
      cols?: number;
      rows?: number;
      activate?: boolean;
    };
    result: { tabId: TabId; paneId: PaneId; sessionId: SessionId };
  };
  "terminal.close": {
    args: {
      tabId?: TabId;
    };
    result: { closedTabId: TabId };
  };
  "terminal.focus": {
    args: {
      tabId?: TabId;
    };
    result: { focusedTabId: TabId };
  };
  "terminal.resync": {
    args: {
      tabId?: TabId;
    };
    result: { resyncedTabId: TabId };
  };
  "terminal.copySelection": {
    args: {
      tabId?: TabId;
    };
    result: { copiedLength: number };
  };
  "tab.select": {
    args: {
      tabId: TabId;
    };
    result: { activeTabId: TabId };
  };
  "app.setLocale": {
    args: {
      locale: Locale;
    };
    result: { locale: Locale };
  };
  "app.setRenderer": {
    args: {
      renderer: TerminalBackendId;
    };
    result: { renderer: TerminalBackendId };
  };
  "app.toggleSidebar": {
    args: Record<string, never>;
    result: { sidebarVisible: boolean };
  };
}

export type BuiltinActionId = keyof BuiltinActionMap;

/**
 * Action IDs accept built-in IDs with full autocomplete as well as custom namespaced IDs
 * (e.g. `"user.myAction"` for Phase 4 TypeScript config extensions).
 */
export type ActionId = BuiltinActionId | (string & {});

export type ActionArgsOf<TId extends ActionId> = TId extends BuiltinActionId
  ? BuiltinActionMap[TId]["args"]
  : unknown;

export type ActionResultOf<TId extends ActionId> = TId extends BuiltinActionId
  ? BuiltinActionMap[TId]["result"]
  : unknown;

export interface ActionDefinition<
  TId extends ActionId = ActionId,
  TArgs = ActionArgsOf<TId>,
  TResult = ActionResultOf<TId>,
> {
  readonly id: TId;
  readonly owner: ActionOwner;
  readonly titleKey: TranslationKey;
  readonly descriptionKey?: TranslationKey;
  /** Valibot schema validating runtime arguments from GUI / CLI / keybinds / TS config. */
  readonly argsSchema: v.BaseSchema<unknown, TArgs, v.BaseIssue<unknown>>;
  /** Optional availability predicate evaluated before execution and for UI enablement. */
  readonly canExecute?: (args: TArgs | undefined, ctx: ActionContext) => ActionAvailability;
  readonly execute: (args: TArgs, ctx: ActionContext) => Promise<TResult> | TResult;
}

export interface ActionDescriptor {
  readonly id: ActionId;
  readonly owner: ActionOwner;
  readonly titleKey: TranslationKey;
  readonly descriptionKey?: TranslationKey;
}
