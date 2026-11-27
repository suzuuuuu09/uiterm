import { describe, expect, it } from "vite-plus/test";
import type { TerminalInfo } from "../../ipc/types.ts";
import { createAppStore, DEFAULT_WINDOW_ID, DEFAULT_WORKSPACE_ID } from "../appStore.ts";

function makeTerminalInfo(overrides: Partial<TerminalInfo> = {}): TerminalInfo {
  return {
    address: "t1",
    name: "term-1",
    argv: ["/bin/fish"],
    cwd: "/workspace/uiterm",
    state: "live",
    cols: 120,
    rows: 40,
    title: "fish",
    pid: 4242,
    createdMs: 1_700_000_000_000,
    finishedMs: null,
    exitCode: null,
    signal: null,
    reason: null,
    status: null,
    statusMs: null,
    turn: 0,
    labels: {},
    foreground: null,
    ...overrides,
  };
}

describe("appStore (Zustand Vanilla, React-independent)", () => {
  it("initializes Window -> Workspace -> Tab -> Pane hierarchy with no terminal output fields", () => {
    const store = createAppStore();
    const state = store.getState();

    expect(state.windows[DEFAULT_WINDOW_ID]).toBeDefined();
    expect(state.workspaces[DEFAULT_WORKSPACE_ID]).toBeDefined();
    expect(state.selection).toEqual({
      activeWindowId: DEFAULT_WINDOW_ID,
      activeWorkspaceId: DEFAULT_WORKSPACE_ID,
      activeTabId: null,
      activePaneId: null,
    });

    // Strict invariant: no terminal output / VT state / render data in Zustand
    expect("terminalOutput" in state).toBe(false);
    expect("vtState" in state).toBe(false);
    expect("scrollbackBuffer" in state).toBe(false);
  });

  it("manages workspaces, tabs, panes, selection, and session bindings", () => {
    const store = createAppStore();

    store.getState().createWorkspace({
      id: "ws-project",
      name: "uiterm",
      rootPath: "/repo/uiterm",
      activate: true,
    });
    expect(store.getState().selection.activeWorkspaceId).toBe("ws-project");

    store.getState().addTerminalTab({
      tabId: "tab-1",
      title: "Shell 1",
      sessionId: "t1",
      cols: 100,
      rows: 30,
      activate: true,
    });
    store.getState().addTerminalTab({
      tabId: "tab-2",
      title: "Shell 2",
      activate: false,
    });

    expect(store.getState().workspaces["ws-project"]?.tabIds).toEqual(["tab-1", "tab-2"]);
    expect(store.getState().selection.activeTabId).toBe("tab-1");
    expect(store.getState().panes["tab-1"]?.sessionId).toBe("t1");

    store.getState().bindPaneSession("tab-2", "t2");
    expect(store.getState().panes["tab-2"]?.sessionId).toBe("t2");

    store.getState().selectTab("tab-2");
    expect(store.getState().selection.activeTabId).toBe("tab-2");
    expect(store.getState().selection.activePaneId).toBe("tab-2");

    // Removing active tab falls back to remaining tab
    store.getState().removeTab("tab-2");
    expect(store.getState().tabs["tab-2"]).toBeUndefined();
    expect(store.getState().panes["tab-2"]).toBeUndefined();
    expect(store.getState().selection.activeTabId).toBe("tab-1");
  });

  it("synchronizes authoritative herdr-core snapshots, ignores stale seq, and handles missed events", () => {
    const store = createAppStore();

    store.getState().applyCoreSnapshot([makeTerminalInfo({ address: "t1", title: "initial" })], 10);
    expect(store.getState().coreConnection.status).toBe("connected");
    expect(store.getState().coreConnection.lastSeq).toBe(10);
    expect(store.getState().sessions["t1"]?.title).toBe("initial");

    // Stale event (seq <= 10) must be discarded
    store.getState().applyCoreEvent({
      type: "title",
      seq: 9,
      terminal: "t1",
      data: { title: "stale-title" },
    });
    expect(store.getState().sessions["t1"]?.title).toBe("initial");

    // Newer event updates mirrored session state
    store.getState().applyCoreEvent({
      type: "title",
      seq: 11,
      terminal: "t1",
      data: { title: "nvim" },
    });
    expect(store.getState().sessions["t1"]?.title).toBe("nvim");
    expect(store.getState().coreConnection.lastSeq).toBe(11);

    // Missed event with equal seq must be ignored
    store.getState().applyCoreEvent({
      type: "missed",
      seq: 11,
      terminals: [makeTerminalInfo({ address: "t-stale", title: "should-not-apply" })],
    });
    expect(store.getState().sessions["t1"]?.title).toBe("nvim");
    expect(store.getState().sessions["t-stale"]).toBeUndefined();

    // Missed event replaces full snapshot and advances seq
    store.getState().applyCoreEvent({
      type: "missed",
      seq: 25,
      terminals: [makeTerminalInfo({ address: "t2", title: "recovered" })],
    });
    expect(store.getState().sessions["t1"]).toBeUndefined();
    expect(store.getState().sessions["t2"]?.title).toBe("recovered");
    expect(store.getState().coreConnection.lastSeq).toBe(25);

    // Disconnected event marks connection disconnected
    store.getState().applyCoreEvent({
      type: "disconnected",
      reason: "eof",
    });
    expect(store.getState().coreConnection.status).toBe("disconnected");
  });

  it("parses raw herdr-core snake_case finished payloads and ignores malformed unknown event data", () => {
    const store = createAppStore();
    store.getState().applyCoreSnapshot([makeTerminalInfo({ address: "t1", title: "fish" })], 1);

    // Malformed unknown data should advance seq without crashing or corrupting state
    store.getState().applyCoreEvent({
      type: "title",
      seq: 2,
      terminal: "t1",
      data: { unexpected: 123 },
    });
    expect(store.getState().sessions["t1"]?.title).toBe("fish");
    expect(store.getState().coreConnection.lastSeq).toBe(2);

    // Real herdr-core wire payload uses snake_case `exit_code` and `finished_ms`
    store.getState().applyCoreEvent({
      type: "finished",
      seq: 3,
      terminal: "t1",
      data: { reason: "exited", exit_code: 0, signal: null, finished_ms: 1_700_000_005_000 },
    });
    expect(store.getState().sessions["t1"]).toMatchObject({
      state: "finished",
      reason: "exited",
      exitCode: 0,
      signal: null,
    });
  });

  it("normalizes OSC 7501 status and turn events into AgentState", () => {
    const store = createAppStore();
    store.getState().applyCoreSnapshot([makeTerminalInfo({ address: "t1" })], 1);

    store.getState().applyCoreEvent({
      type: "status",
      seq: 2,
      terminal: "t1",
      data: {
        status: { state: "working", app: "codex", title: "Refactoring", progress: 40 },
        records: {},
        reported: true,
      },
    });
    expect(store.getState().agentStates["t1"]).toMatchObject({
      sessionId: "t1",
      status: "working",
      app: "codex",
      title: "Refactoring",
      progress: 40,
    });

    store.getState().applyCoreEvent({
      type: "turn",
      seq: 3,
      terminal: "t1",
      data: { turn: 2, state: "waiting_approval" },
    });
    expect(store.getState().agentStates["t1"]).toMatchObject({
      status: "waiting_approval",
      app: "codex",
      turn: 2,
    });

    // Restart without status clears stale agent state
    store.getState().applyCoreEvent({
      type: "restarted",
      seq: 4,
      terminal: "t1",
      data: makeTerminalInfo({ address: "t1", status: null }),
    });
    expect(store.getState().agentStates["t1"]).toBeUndefined();

    // Finished clears active agent state
    store.getState().applyCoreEvent({
      type: "status",
      seq: 5,
      terminal: "t1",
      data: { status: { state: "working", app: "codex" }, reported: true },
    });
    expect(store.getState().agentStates["t1"]?.status).toBe("working");
    store.getState().applyCoreEvent({
      type: "finished",
      seq: 6,
      terminal: "t1",
      data: { reason: "exited", exit_code: 0, signal: null },
    });
    expect(store.getState().agentStates["t1"]).toBeUndefined();
  });

  it("keeps window activeWorkspaceId in sync and rejects duplicate tab/pane IDs", () => {
    const store = createAppStore();
    store.getState().createWorkspace({ id: "ws-2", name: "Second", activate: false });
    expect(store.getState().windows[DEFAULT_WINDOW_ID]?.activeWorkspaceId).toBe(
      DEFAULT_WORKSPACE_ID,
    );

    store.getState().addTerminalTab({
      tabId: "tab-ws2",
      workspaceId: "ws-2",
      title: "WS2 Shell",
      activate: true,
    });
    expect(store.getState().windows[DEFAULT_WINDOW_ID]?.activeWorkspaceId).toBe("ws-2");

    expect(() =>
      store.getState().addTerminalTab({
        tabId: "tab-ws2",
        workspaceId: "ws-2",
        title: "Duplicate",
      }),
    ).toThrow(/already exists/i);
  });

  it("updates UI state and user settings without redundant mutations", () => {
    const store = createAppStore();
    const uiBefore = store.getState().ui;
    store.getState().setIsComposing(false);
    expect(store.getState().ui).toBe(uiBefore);

    expect(store.getState().ui.sidebarVisible).toBe(false);
    expect(store.getState().ui.diagnosticsPanelVisible).toBe(false);

    store.getState().toggleSidebar();
    expect(store.getState().ui.sidebarVisible).toBe(true);

    store.getState().toggleDiagnosticsPanel();
    expect(store.getState().ui.diagnosticsPanelVisible).toBe(true);

    store.getState().updateSettings({ locale: "en", fontSize: 16 });
    expect(store.getState().settings.locale).toBe("en");
    expect(store.getState().settings.fontSize).toBe(16);
  });
});
