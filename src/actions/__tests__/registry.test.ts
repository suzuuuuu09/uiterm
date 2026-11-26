import * as v from "valibot";
import { describe, expect, it, vi } from "vite-plus/test";
import { createAppRuntime } from "../../app/runtime.ts";
import { MockIpcClient } from "../../ipc/mock.ts";
import { Logger } from "../../logging/logger.ts";
import type { TerminalAdapter, TerminalAdapterCallbacks } from "../../terminal/types.ts";
import { ActionRegistry } from "../registry.ts";

function createFakeElement(): HTMLDivElement {
  return {
    dataset: {},
    className: "",
    style: { display: "" },
    appendChild(child: unknown) {
      return child;
    },
    remove() {},
  } as unknown as HTMLDivElement;
}

function createStubAdapterFactory(selectionText = "selected text") {
  return (_backendId: unknown, _callbacks: TerminalAdapterCallbacks): TerminalAdapter => ({
    backendId: "wterm-ghostty-dom",
    cols: 80,
    rows: 24,
    async mount() {},
    write() {},
    resize() {},
    fit() {},
    focus() {},
    setHidden() {},
    async getSelection() {
      return selectionText;
    },
    dispose() {},
  });
}

describe("ActionRegistry", () => {
  it("registers, validates args with Valibot, checks canExecute, and translates labels", async () => {
    const logger = new Logger({ minLevel: "debug" });
    const registry = new ActionRegistry(logger);
    let enabled = false;

    registry.register({
      id: "custom.echo",
      owner: "app",
      titleKey: "action.app.toggleSidebar.title",
      descriptionKey: "action.app.toggleSidebar.description",
      argsSchema: v.object({ count: v.pipe(v.number(), v.integer(), v.minValue(1)) }),
      canExecute: () =>
        enabled ? { allowed: true } : { allowed: false, reasonKey: "action.unavailable.busy" },
      execute: (args) => ({ doubled: args.count * 2 }),
    });

    expect(registry.has("custom.echo")).toBe(true);
    expect(registry.getLabel("custom.echo", "ja")?.title).toBe("サイドバー表示切替");
    expect(registry.getLabel("custom.echo", "en")?.title).toBe("Toggle Sidebar");

    // Omitted args on required-arg action must be rejected by schema validation, not availability
    expect(registry.canExecute("custom.echo")).toMatchObject({
      allowed: false,
      reasonKey: "error.action.invalidArgs",
    });

    // Invalid args rejected before execution
    const invalid = await registry.execute("custom.echo", { count: 0 }, { caller: "cli" });
    expect(invalid.ok).toBe(false);
    if (!invalid.ok) {
      expect(invalid.error.code).toBe("ACTION_INVALID_ARGS");
    }

    // Unavailable action rejected with structured AppError
    expect(registry.canExecute("custom.echo", { count: 2 }).allowed).toBe(false);
    const unavailable = await registry.execute("custom.echo", { count: 2 }, { caller: "keybind" });
    expect(unavailable.ok).toBe(false);
    if (!unavailable.ok) {
      expect(unavailable.error.code).toBe("ACTION_UNAVAILABLE");
      expect(unavailable.error.messageKey).toBe("action.unavailable.busy");
    }

    // Enabled execution succeeds
    enabled = true;
    const ok = await registry.execute("custom.echo", { count: 3 }, { caller: "config" });
    expect(ok).toEqual({ ok: true, value: { doubled: 6 } });

    // Unknown action returns ACTION_NOT_FOUND
    const missing = await registry.execute("unknown.action", {}, { caller: "cli" });
    expect(missing.ok).toBe(false);
    if (!missing.ok) {
      expect(missing.error.code).toBe("ACTION_NOT_FOUND");
    }
  });

  it("normalizes handler exceptions into structured AppError", async () => {
    const logger = new Logger({ minLevel: "debug" });
    const registry = new ActionRegistry(logger);

    registry.register({
      id: "custom.fail",
      owner: "app",
      titleKey: "action.app.setLocale.title",
      argsSchema: v.object({}),
      execute: () => {
        throw new Error("boom");
      },
    });

    const result = await registry.execute("custom.fail", {}, { caller: "hook" });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error.code).toBe("ACTION_EXECUTION_FAILED");
      expect(result.error.message).toContain("boom");
    }

    // Throwing canExecute must also be normalized into ActionResult
    registry.register({
      id: "custom.failCanExecute",
      owner: "app",
      titleKey: "action.app.setLocale.title",
      argsSchema: v.object({}),
      canExecute: () => {
        throw new Error("availability boom");
      },
      execute: () => ({}),
    });
    const availFail = await registry.execute("custom.failCanExecute", {}, { caller: "gui" });
    expect(availFail.ok).toBe(false);
    if (!availFail.ok) {
      expect(availFail.error.code).toBe("ACTION_EXECUTION_FAILED");
      expect(availFail.error.message).toContain("availability boom");
    }
  });

  it("wires built-in terminal and app actions to appStore and TerminalSessionManager", async () => {
    vi.stubGlobal("document", { createElement: createFakeElement });
    const copied: string[] = [];
    const runtime = createAppRuntime({
      client: new MockIpcClient(),
      adapterFactory: createStubAdapterFactory("日本語選択"),
      writeClipboard: async (text) => {
        copied.push(text);
      },
    });

    try {
      // Cannot create terminal before host is mounted
      expect(runtime.actions.canExecute("terminal.create", {}).allowed).toBe(false);

      runtime.setMountRoot(createFakeElement());
      expect(runtime.actions.canExecute("terminal.create", {}).allowed).toBe(true);

      const created = await runtime.actions.execute(
        "terminal.create",
        { title: "Main Shell" },
        { caller: "gui" },
      );
      expect(created.ok).toBe(true);
      if (!created.ok) return;

      const { tabId, sessionId } = created.value;
      expect(runtime.store.getState().tabs[tabId]?.title).toBe("Main Shell");
      expect(runtime.store.getState().panes[tabId]?.sessionId).toBe(sessionId);

      // Copy selection
      expect(
        runtime.actions.canExecute("terminal.copySelection", { tabId: "non-existent" }).allowed,
      ).toBe(false);
      expect(runtime.actions.canExecute("tab.select").allowed).toBe(false);
      expect(runtime.actions.canExecute("tab.select", { tabId: "toString" }).allowed).toBe(false);
      const copyRes = await runtime.actions.execute("terminal.copySelection", {}, { caller: "keybind" });
      expect(copyRes).toEqual({ ok: true, value: { copiedLength: 5 } });
      expect(copied).toEqual(["日本語選択"]);

      // Resync active terminal
      const resyncRes = await runtime.actions.execute("terminal.resync", {}, { caller: "cli" });
      expect(resyncRes).toEqual({ ok: true, value: { resyncedTabId: tabId } });

      // Locale & Sidebar actions
      const localeRes = await runtime.actions.execute(
        "app.setLocale",
        { locale: "en" },
        { caller: "config" },
      );
      expect(localeRes).toEqual({ ok: true, value: { locale: "en" } });
      expect(runtime.store.getState().settings.locale).toBe("en");

      const sidebarRes = await runtime.actions.execute("app.toggleSidebar", {}, { caller: "keybind" });
      expect(sidebarRes).toEqual({ ok: true, value: { sidebarVisible: true } });

      const diagRes = await runtime.actions.execute(
        "app.toggleDiagnostics",
        {},
        { caller: "keybind" },
      );
      expect(diagRes).toEqual({ ok: true, value: { diagnosticsPanelVisible: true } });

      // Close terminal
      const closed = await runtime.actions.execute("terminal.close", { tabId }, { caller: "gui" });
      expect(closed).toEqual({ ok: true, value: { closedTabId: tabId } });
      expect(runtime.store.getState().tabs[tabId]).toBeUndefined();
      expect(runtime.sessionManager.get(tabId)).toBeUndefined();
    } finally {
      runtime.dispose();
      vi.unstubAllGlobals();
    }
  });
});
