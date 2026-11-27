import type { AppError } from "../errors/appError.ts";
import type { CoreStatus, ForegroundProcess, SessionId, StatusRecord } from "../ipc/types.ts";
import type { UserSettings } from "../validation/settingsSchemas.ts";

export type WindowId = string;
export type WorkspaceId = string;
export type TabId = string;
export type PaneId = string;

export type TabKind = "terminal";

export interface WindowState {
  readonly id: WindowId;
  readonly title: string;
  readonly workspaceIds: readonly WorkspaceId[];
  readonly activeWorkspaceId: WorkspaceId | null;
  readonly focused: boolean;
}

export interface WorkspaceState {
  readonly id: WorkspaceId;
  readonly windowId: WindowId;
  readonly name: string;
  readonly rootPath: string | null;
  readonly gitBranch: string | null;
  readonly tabIds: readonly TabId[];
  readonly activeTabId: TabId | null;
  readonly createdAt: number;
}

export interface TabState {
  readonly id: TabId;
  readonly workspaceId: WorkspaceId;
  readonly kind: TabKind;
  readonly title: string;
  readonly paneIds: readonly PaneId[];
  readonly activePaneId: PaneId | null;
  readonly createdAt: number;
}

export interface PaneState {
  readonly id: PaneId;
  readonly tabId: TabId;
  /** Associated herdr-core session ID. Null while creating or detached from a backend session. */
  readonly sessionId: SessionId | null;
  readonly title: string;
  readonly cols: number;
  readonly rows: number;
  readonly createdAt: number;
}

export interface SelectionState {
  readonly activeWindowId: WindowId | null;
  readonly activeWorkspaceId: WorkspaceId | null;
  readonly activeTabId: TabId | null;
  readonly activePaneId: PaneId | null;
}

/**
 * Authoritative session execution metadata mirrored from `herdr-core`.
 * Terminal output bytes, VT cell grids, and scrollback NEVER enter Zustand.
 */
export interface CoreSessionSnapshot {
  readonly sessionId: SessionId;
  readonly name: string | null;
  readonly state: "live" | "finished" | "lost" | string;
  readonly title: string;
  readonly cwd: string;
  readonly cols: number;
  readonly rows: number;
  readonly pid: number | null;
  readonly exitCode: number | null;
  readonly signal: string | null;
  readonly reason: string | null;
  readonly turn: number;
  readonly labels: Readonly<Record<string, string>>;
  readonly foreground: ForegroundProcess | null;
  readonly rawStatus: StatusRecord | null;
  readonly updatedAt: number;
}

export type CoreConnectionStatus = "disconnected" | "connected" | "reconnecting";

export interface CoreConnectionState {
  readonly status: CoreConnectionStatus;
  readonly lastSeq: number;
  readonly coreStatus: CoreStatus | null;
  readonly lastError: AppError | null;
}

/**
 * Normalized AI agent state derived from `herdr-core` OSC 7501 / turn events.
 */
export type AgentStatusKind =
  | "idle"
  | "working"
  | "waiting_input"
  | "waiting_approval"
  | "done"
  | "error";

export interface AgentState {
  readonly sessionId: SessionId;
  readonly status: AgentStatusKind;
  readonly app: string | null;
  readonly title: string | null;
  readonly message: string | null;
  readonly progress: number | null;
  readonly turn: number;
  readonly updatedAt: number;
}

export interface UiNotification {
  readonly id: number;
  readonly severity: "info" | "warn" | "error";
  readonly message: string;
  readonly timestamp: number;
}

export interface UiState {
  readonly sidebarVisible: boolean;
  readonly isComposing: boolean;
  readonly diagnosticsPanelVisible: boolean;
  readonly lastNotification: UiNotification | null;
}

export interface AppStateData {
  readonly windows: Readonly<Record<WindowId, WindowState>>;
  readonly workspaces: Readonly<Record<WorkspaceId, WorkspaceState>>;
  readonly tabs: Readonly<Record<TabId, TabState>>;
  readonly panes: Readonly<Record<PaneId, PaneState>>;
  readonly selection: SelectionState;
  readonly sessions: Readonly<Record<SessionId, CoreSessionSnapshot>>;
  readonly coreConnection: CoreConnectionState;
  readonly agentStates: Readonly<Record<SessionId, AgentState>>;
  readonly ui: UiState;
  readonly settings: UserSettings;
}
