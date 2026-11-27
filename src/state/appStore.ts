import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";
import type { AppError } from "../errors/appError.ts";
import type { CoreEvent, CoreStatus, SessionId, TerminalInfo } from "../ipc/types.ts";
import { DEFAULT_USER_SETTINGS, type UserSettings } from "../validation/settingsSchemas.ts";
import { applyCoreEvent, applyCoreSnapshot } from "./coreSync.ts";
import type {
  AppStateData,
  CoreConnectionStatus,
  PaneId,
  PaneState,
  TabId,
  TabState,
  WindowId,
  WindowState,
  WorkspaceId,
  WorkspaceState,
} from "./types.ts";

interface CreateTerminalTabInput {
  readonly tabId: TabId;
  readonly paneId?: PaneId;
  readonly workspaceId?: WorkspaceId;
  readonly title: string;
  readonly sessionId?: SessionId | null;
  readonly cols?: number;
  readonly rows?: number;
  readonly activate?: boolean;
}

interface AppStoreActions {
  // Workspace / Window hierarchy
  createWorkspace: (input: {
    id: WorkspaceId;
    name: string;
    rootPath?: string | null;
    windowId?: WindowId;
    activate?: boolean;
  }) => WorkspaceState;
  selectWorkspace: (workspaceId: WorkspaceId) => void;

  // Tab & Pane lifecycle (layout & view state owned by uiterm)
  addTerminalTab: (input: CreateTerminalTabInput) => { tab: TabState; pane: PaneState };
  removeTab: (tabId: TabId) => void;
  clearWorkspaceTabs: (workspaceId?: WorkspaceId) => void;
  selectTab: (tabId: TabId | null) => void;
  selectPane: (paneId: PaneId) => void;
  bindPaneSession: (paneId: PaneId, sessionId: SessionId | null) => void;
  updatePaneSize: (paneId: PaneId, cols: number, rows: number) => void;
  updateTabTitle: (tabId: TabId, title: string) => void;

  // Authoritative herdr-core session & event synchronization
  setCoreConnection: (patch: {
    status?: CoreConnectionStatus;
    coreStatus?: CoreStatus | null;
    lastError?: AppError | null;
  }) => void;
  applyCoreSnapshot: (terminals: readonly TerminalInfo[], seq: number) => void;
  applyCoreEvent: (event: CoreEvent) => void;

  // UI state
  setSidebarVisible: (visible: boolean) => void;
  toggleSidebar: () => void;
  setIsComposing: (isComposing: boolean) => void;
  setDiagnosticsPanelVisible: (visible: boolean) => void;
  toggleDiagnosticsPanel: () => void;
  pushNotification: (severity: "info" | "warn" | "error", message: string) => void;
  clearNotification: () => void;

  // User settings
  updateSettings: (patch: Partial<UserSettings>) => void;

  // Reset (for tests)
  reset: () => void;
}

export type AppState = AppStateData & AppStoreActions;

export const DEFAULT_WINDOW_ID: WindowId = "win-main";
export const DEFAULT_WORKSPACE_ID: WorkspaceId = "ws-default";

function createInitialData(initialSettings?: Partial<UserSettings>): AppStateData {
  const now = Date.now();
  const mainWindow: WindowState = {
    id: DEFAULT_WINDOW_ID,
    title: "uiterm",
    workspaceIds: [DEFAULT_WORKSPACE_ID],
    activeWorkspaceId: DEFAULT_WORKSPACE_ID,
    focused: true,
  };
  const defaultWorkspace: WorkspaceState = {
    id: DEFAULT_WORKSPACE_ID,
    windowId: DEFAULT_WINDOW_ID,
    name: "Default",
    rootPath: null,
    gitBranch: null,
    tabIds: [],
    activeTabId: null,
    createdAt: now,
  };

  return {
    windows: { [DEFAULT_WINDOW_ID]: mainWindow },
    workspaces: { [DEFAULT_WORKSPACE_ID]: defaultWorkspace },
    tabs: {},
    panes: {},
    selection: {
      activeWindowId: DEFAULT_WINDOW_ID,
      activeWorkspaceId: DEFAULT_WORKSPACE_ID,
      activeTabId: null,
      activePaneId: null,
    },
    sessions: {},
    coreConnection: {
      status: "disconnected",
      lastSeq: 0,
      coreStatus: null,
      lastError: null,
    },
    agentStates: {},
    ui: {
      sidebarVisible: false,
      isComposing: false,
      diagnosticsPanelVisible: false,
      lastNotification: null,
    },
    settings: {
      ...DEFAULT_USER_SETTINGS,
      ...initialSettings,
    },
  };
}

/**
 * Creates a React-independent Zustand Vanilla store for uiterm application state.
 *
 * Ownership boundaries:
 * - `uiterm` owns Window -> Workspace -> Tab -> Pane layout, selection, UI flags, and settings.
 * - `herdr-core` owns session execution state; mirrored here via `applyCoreSnapshot`/`applyCoreEvent`.
 * - Terminal output bytes, VT state, and rendering buffers NEVER enter this store.
 */
export function createAppStore(initialSettings?: Partial<UserSettings>): StoreApi<AppState> {
  let nextNotificationId = 1;

  return createStore<AppState>((set, get) => ({
    ...createInitialData(initialSettings),

    createWorkspace: ({ id, name, rootPath = null, windowId, activate = false }) => {
      const state = get();
      const targetWindowId = windowId ?? state.selection.activeWindowId ?? DEFAULT_WINDOW_ID;
      const win = state.windows[targetWindowId];
      if (!win) {
        throw new Error(`Window not found: ${targetWindowId}`);
      }
      const workspace: WorkspaceState = {
        id,
        windowId: targetWindowId,
        name,
        rootPath,
        gitBranch: null,
        tabIds: [],
        activeTabId: null,
        createdAt: Date.now(),
      };
      set((s) => ({
        workspaces: { ...s.workspaces, [id]: workspace },
        windows: {
          ...s.windows,
          [targetWindowId]: {
            ...win,
            workspaceIds: [...win.workspaceIds, id],
            activeWorkspaceId: activate ? id : win.activeWorkspaceId,
          },
        },
        selection: activate
          ? {
              activeWindowId: targetWindowId,
              activeWorkspaceId: id,
              activeTabId: null,
              activePaneId: null,
            }
          : s.selection,
      }));
      return workspace;
    },

    selectWorkspace: (workspaceId) =>
      set((s) => {
        const ws = s.workspaces[workspaceId];
        if (!ws) return s;
        const win = s.windows[ws.windowId];
        const activeTab = ws.activeTabId ? s.tabs[ws.activeTabId] : undefined;
        return {
          windows: win
            ? {
                ...s.windows,
                [ws.windowId]: { ...win, activeWorkspaceId: workspaceId },
              }
            : s.windows,
          selection: {
            activeWindowId: ws.windowId,
            activeWorkspaceId: workspaceId,
            activeTabId: activeTab?.id ?? null,
            activePaneId: activeTab?.activePaneId ?? null,
          },
        };
      }),

    addTerminalTab: (input) => {
      const state = get();
      const workspaceId =
        input.workspaceId ?? state.selection.activeWorkspaceId ?? DEFAULT_WORKSPACE_ID;
      const workspace = state.workspaces[workspaceId];
      if (!workspace) {
        throw new Error(`Workspace not found: ${workspaceId}`);
      }

      const now = Date.now();
      const paneId = input.paneId ?? input.tabId;
      if (state.tabs[input.tabId]) {
        throw new Error(`Tab already exists: ${input.tabId}`);
      }
      if (state.panes[paneId]) {
        throw new Error(`Pane already exists: ${paneId}`);
      }
      const pane: PaneState = {
        id: paneId,
        tabId: input.tabId,
        sessionId: input.sessionId ?? null,
        title: input.title,
        cols: input.cols ?? 80,
        rows: input.rows ?? 24,
        createdAt: now,
      };
      const tab: TabState = {
        id: input.tabId,
        workspaceId,
        kind: "terminal",
        title: input.title,
        paneIds: [paneId],
        activePaneId: paneId,
        createdAt: now,
      };

      const shouldActivate = input.activate ?? workspace.tabIds.length === 0;

      set((s) => {
        const ws = s.workspaces[workspaceId]!;
        const win = shouldActivate ? s.windows[ws.windowId] : undefined;
        const nextActiveTabId = shouldActivate ? tab.id : ws.activeTabId;
        return {
          tabs: { ...s.tabs, [tab.id]: tab },
          panes: { ...s.panes, [pane.id]: pane },
          workspaces: {
            ...s.workspaces,
            [workspaceId]: {
              ...ws,
              tabIds: [...ws.tabIds, tab.id],
              activeTabId: nextActiveTabId,
            },
          },
          windows: win
            ? {
                ...s.windows,
                [ws.windowId]: { ...win, activeWorkspaceId: workspaceId },
              }
            : s.windows,
          selection: shouldActivate
            ? {
                activeWindowId: ws.windowId,
                activeWorkspaceId: workspaceId,
                activeTabId: tab.id,
                activePaneId: pane.id,
              }
            : s.selection,
        };
      });

      return { tab, pane };
    },

    removeTab: (tabId) =>
      set((s) => {
        const tab = s.tabs[tabId];
        if (!tab) return s;

        const ws = s.workspaces[tab.workspaceId];
        const nextTabs = { ...s.tabs };
        delete nextTabs[tabId];

        const nextPanes = { ...s.panes };
        for (const paneId of tab.paneIds) {
          delete nextPanes[paneId];
        }

        const remainingTabIds = ws ? ws.tabIds.filter((id) => id !== tabId) : [];
        const nextWsActiveTabId =
          ws && ws.activeTabId === tabId
            ? (remainingTabIds[remainingTabIds.length - 1] ?? null)
            : (ws?.activeTabId ?? null);

        const nextWorkspaces = ws
          ? {
              ...s.workspaces,
              [ws.id]: {
                ...ws,
                tabIds: remainingTabIds,
                activeTabId: nextWsActiveTabId,
              },
            }
          : s.workspaces;

        let nextSelection = s.selection;
        if (s.selection.activeTabId === tabId) {
          const fallbackTab = nextWsActiveTabId ? nextTabs[nextWsActiveTabId] : undefined;
          nextSelection = {
            ...s.selection,
            activeTabId: fallbackTab?.id ?? null,
            activePaneId: fallbackTab?.activePaneId ?? null,
          };
        }

        return {
          tabs: nextTabs,
          panes: nextPanes,
          workspaces: nextWorkspaces,
          selection: nextSelection,
        };
      }),

    clearWorkspaceTabs: (workspaceId) =>
      set((s) => {
        const targetWsId = workspaceId ?? s.selection.activeWorkspaceId ?? DEFAULT_WORKSPACE_ID;
        const ws = s.workspaces[targetWsId];
        if (!ws || ws.tabIds.length === 0) return s;

        const nextTabs = { ...s.tabs };
        const nextPanes = { ...s.panes };
        for (const tabId of ws.tabIds) {
          const tab = nextTabs[tabId];
          if (tab) {
            for (const paneId of tab.paneIds) {
              delete nextPanes[paneId];
            }
            delete nextTabs[tabId];
          }
        }

        return {
          tabs: nextTabs,
          panes: nextPanes,
          workspaces: {
            ...s.workspaces,
            [targetWsId]: {
              ...ws,
              tabIds: [],
              activeTabId: null,
            },
          },
          selection:
            s.selection.activeWorkspaceId === targetWsId
              ? { ...s.selection, activeTabId: null, activePaneId: null }
              : s.selection,
        };
      }),

    selectTab: (tabId) =>
      set((s) => {
        if (tabId === null) {
          if (s.selection.activeTabId === null && s.selection.activePaneId === null) return s;
          const wsId = s.selection.activeWorkspaceId;
          const ws = wsId ? s.workspaces[wsId] : undefined;
          return {
            workspaces: ws
              ? { ...s.workspaces, [ws.id]: { ...ws, activeTabId: null } }
              : s.workspaces,
            selection: { ...s.selection, activeTabId: null, activePaneId: null },
          };
        }

        const tab = s.tabs[tabId];
        if (!tab) return s;
        if (
          s.selection.activeTabId === tab.id &&
          s.selection.activePaneId === tab.activePaneId &&
          s.selection.activeWorkspaceId === tab.workspaceId
        ) {
          return s;
        }
        const ws = s.workspaces[tab.workspaceId];
        const win = ws ? s.windows[ws.windowId] : undefined;
        return {
          windows:
            ws && win
              ? { ...s.windows, [ws.windowId]: { ...win, activeWorkspaceId: ws.id } }
              : s.windows,
          workspaces: ws
            ? { ...s.workspaces, [ws.id]: { ...ws, activeTabId: tab.id } }
            : s.workspaces,
          selection: {
            activeWindowId: ws?.windowId ?? s.selection.activeWindowId,
            activeWorkspaceId: tab.workspaceId,
            activeTabId: tab.id,
            activePaneId: tab.activePaneId,
          },
        };
      }),

    selectPane: (paneId) =>
      set((s) => {
        const pane = s.panes[paneId];
        if (!pane) return s;
        const tab = s.tabs[pane.tabId];
        if (!tab) return s;
        if (s.selection.activePaneId === paneId && s.selection.activeTabId === tab.id) {
          return s;
        }
        const ws = s.workspaces[tab.workspaceId];
        const win = ws ? s.windows[ws.windowId] : undefined;
        return {
          tabs: {
            ...s.tabs,
            [tab.id]: { ...tab, activePaneId: paneId },
          },
          windows:
            ws && win
              ? { ...s.windows, [ws.windowId]: { ...win, activeWorkspaceId: ws.id } }
              : s.windows,
          workspaces: ws
            ? { ...s.workspaces, [ws.id]: { ...ws, activeTabId: tab.id } }
            : s.workspaces,
          selection: {
            activeWindowId: ws?.windowId ?? s.selection.activeWindowId,
            activeWorkspaceId: tab.workspaceId,
            activeTabId: tab.id,
            activePaneId: paneId,
          },
        };
      }),

    bindPaneSession: (paneId, sessionId) =>
      set((s) => {
        const pane = s.panes[paneId];
        if (!pane || pane.sessionId === sessionId) return s;
        return {
          panes: {
            ...s.panes,
            [paneId]: { ...pane, sessionId },
          },
        };
      }),

    updatePaneSize: (paneId, cols, rows) =>
      set((s) => {
        const pane = s.panes[paneId];
        if (!pane || (pane.cols === cols && pane.rows === rows)) return s;
        return {
          panes: {
            ...s.panes,
            [paneId]: { ...pane, cols, rows },
          },
        };
      }),

    updateTabTitle: (tabId, title) =>
      set((s) => {
        const tab = s.tabs[tabId];
        if (!tab || tab.title === title) return s;
        return {
          tabs: {
            ...s.tabs,
            [tabId]: { ...tab, title },
          },
        };
      }),

    setCoreConnection: (patch) =>
      set((s) => ({
        coreConnection: {
          ...s.coreConnection,
          ...patch,
        },
      })),

    applyCoreSnapshot: (terminals, seq) =>
      set((s) => {
        const next = applyCoreSnapshot(terminals, seq);
        return {
          sessions: next.sessions,
          agentStates: next.agentStates,
          coreConnection: {
            ...s.coreConnection,
            status: "connected",
            lastSeq: next.lastSeq,
            lastError: null,
          },
        };
      }),

    applyCoreEvent: (event) =>
      set((s) => {
        const patch = applyCoreEvent(
          {
            sessions: s.sessions,
            agentStates: s.agentStates,
            lastSeq: s.coreConnection.lastSeq,
          },
          event,
        );
        if (!patch) return s;

        return {
          sessions: patch.sessions,
          agentStates: patch.agentStates,
          coreConnection: {
            ...s.coreConnection,
            status: patch.disconnected ? "disconnected" : s.coreConnection.status,
            lastSeq: patch.lastSeq,
          },
        };
      }),

    setSidebarVisible: (sidebarVisible) =>
      set((s) =>
        s.ui.sidebarVisible === sidebarVisible ? s : { ui: { ...s.ui, sidebarVisible } },
      ),

    toggleSidebar: () =>
      set((s) => ({
        ui: { ...s.ui, sidebarVisible: !s.ui.sidebarVisible },
      })),

    setIsComposing: (isComposing) =>
      set((s) => (s.ui.isComposing === isComposing ? s : { ui: { ...s.ui, isComposing } })),

    setDiagnosticsPanelVisible: (diagnosticsPanelVisible) =>
      set((s) =>
        s.ui.diagnosticsPanelVisible === diagnosticsPanelVisible
          ? s
          : { ui: { ...s.ui, diagnosticsPanelVisible } },
      ),

    toggleDiagnosticsPanel: () =>
      set((s) => ({
        ui: { ...s.ui, diagnosticsPanelVisible: !s.ui.diagnosticsPanelVisible },
      })),

    pushNotification: (severity, message) =>
      set((s) => ({
        ui: {
          ...s.ui,
          lastNotification: {
            id: nextNotificationId++,
            severity,
            message,
            timestamp: Date.now(),
          },
        },
      })),

    clearNotification: () =>
      set((s) =>
        s.ui.lastNotification === null ? s : { ui: { ...s.ui, lastNotification: null } },
      ),

    updateSettings: (patch) =>
      set((s) => ({
        settings: { ...s.settings, ...patch },
      })),

    reset: () => set(createInitialData(initialSettings)),
  }));
}

export const appStore = createAppStore();

export function useAppStore<T>(selector: (state: AppState) => T): T {
  return useStore(appStore, selector);
}
