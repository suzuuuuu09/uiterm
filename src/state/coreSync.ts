import type { CoreEvent, SessionId, StatusRecord, TerminalInfo } from "../ipc/types.ts";
import {
  parseLabelsEventData,
  parseSizeEventData,
  parseStatusEventData,
  parseTerminationEventData,
  parseTitleEventData,
  parseTurnEventData,
} from "../validation/ipcSchemas.ts";
import type { AgentState, AgentStatusKind, CoreSessionSnapshot } from "./types.ts";

export function normalizeAgentStatusKind(rawState: string | undefined): AgentStatusKind {
  switch (rawState?.toLowerCase()) {
    case "working":
    case "running":
    case "busy":
    case "thinking":
      return "working";
    case "waiting_input":
    case "input":
    case "prompt":
    case "needs_input":
      return "waiting_input";
    case "waiting_approval":
    case "approval":
    case "blocked":
    case "permission":
      return "waiting_approval";
    case "done":
    case "completed":
    case "success":
      return "done";
    case "error":
    case "failed":
      return "error";
    default:
      return "idle";
  }
}

export function agentStateFromStatusRecord(
  sessionId: SessionId,
  record: StatusRecord | null,
  turn: number,
  nowMs: number = Date.now(),
): AgentState {
  return {
    sessionId,
    status: normalizeAgentStatusKind(record?.state),
    app: record?.app ? record.app : null,
    title: record?.title ? record.title : null,
    message: record?.msg ? record.msg : null,
    progress: record?.progress ?? null,
    turn,
    updatedAt: nowMs,
  };
}

export function terminalInfoToSnapshot(
  info: TerminalInfo,
  nowMs: number = Date.now(),
): CoreSessionSnapshot {
  return {
    sessionId: info.address,
    name: info.name,
    state: info.state,
    title: info.title,
    cwd: info.cwd,
    cols: info.cols,
    rows: info.rows,
    pid: info.pid,
    exitCode: info.exitCode,
    signal: info.signal,
    reason: info.reason,
    turn: info.turn,
    labels: info.labels,
    foreground: info.foreground,
    rawStatus: info.status,
    updatedAt: nowMs,
  };
}

export interface CoreSyncStateSlice {
  readonly sessions: Readonly<Record<SessionId, CoreSessionSnapshot>>;
  readonly agentStates: Readonly<Record<SessionId, AgentState>>;
  readonly lastSeq: number;
}

export interface CoreSyncPatch {
  readonly sessions: Readonly<Record<SessionId, CoreSessionSnapshot>>;
  readonly agentStates: Readonly<Record<SessionId, AgentState>>;
  readonly lastSeq: number;
  readonly disconnected?: boolean;
}

export function applyCoreSnapshot(
  terminals: readonly TerminalInfo[],
  seq: number,
  nowMs: number = Date.now(),
): CoreSyncPatch {
  const sessions: Record<SessionId, CoreSessionSnapshot> = {};
  const agentStates: Record<SessionId, AgentState> = {};

  for (const info of terminals) {
    sessions[info.address] = terminalInfoToSnapshot(info, nowMs);
    if (info.status) {
      agentStates[info.address] = agentStateFromStatusRecord(
        info.address,
        info.status,
        info.turn,
        nowMs,
      );
    }
  }

  return { sessions, agentStates, lastSeq: seq };
}

/**
 * Pure reducer applying a single `CoreEvent` from `herdr-core` onto the mirrored session
 * and agent state slices.
 *
 * Enforces `docs/poc/interface.md` §6:
 * - Events with `seq <= lastSeq` are discarded as stale (except `disconnected`).
 * - `missed` replaces the full session snapshot with `event.terminals` and advances `lastSeq`.
 */
export function applyCoreEvent(
  current: CoreSyncStateSlice,
  event: CoreEvent,
  nowMs: number = Date.now(),
): CoreSyncPatch | null {
  if (event.type === "disconnected") {
    return {
      sessions: current.sessions,
      agentStates: current.agentStates,
      lastSeq: current.lastSeq,
      disconnected: true,
    };
  }

  if (event.type === "missed") {
    if (event.seq <= current.lastSeq) {
      return null;
    }
    return applyCoreSnapshot(event.terminals, event.seq, nowMs);
  }

  if (event.seq <= current.lastSeq) {
    return null;
  }

  const nextSeq = event.seq;

  switch (event.type) {
    case "heartbeat":
      return {
        sessions: current.sessions,
        agentStates: current.agentStates,
        lastSeq: nextSeq,
      };

    case "created":
    case "restarted": {
      const snap = terminalInfoToSnapshot(event.data, nowMs);
      const nextSessions = { ...current.sessions, [event.terminal]: snap };
      let nextAgentStates: Readonly<Record<SessionId, AgentState>> = current.agentStates;
      if (event.data.status) {
        nextAgentStates = {
          ...current.agentStates,
          [event.terminal]: agentStateFromStatusRecord(
            event.terminal,
            event.data.status,
            event.data.turn,
            nowMs,
          ),
        };
      } else if (event.terminal in current.agentStates) {
        const cleaned = { ...current.agentStates };
        delete cleaned[event.terminal];
        nextAgentStates = cleaned;
      }
      return { sessions: nextSessions, agentStates: nextAgentStates, lastSeq: nextSeq };
    }

    case "finished": {
      const prev = current.sessions[event.terminal];
      const hasAgent = event.terminal in current.agentStates;
      if (!prev && !hasAgent) {
        return { sessions: current.sessions, agentStates: current.agentStates, lastSeq: nextSeq };
      }
      const data = parseTerminationEventData(event.data);
      const nextSessions = prev
        ? {
            ...current.sessions,
            [event.terminal]: {
              ...prev,
              state: "finished",
              reason: data.reason,
              exitCode: data.exitCode,
              signal: data.signal,
              foreground: null,
              updatedAt: nowMs,
            },
          }
        : current.sessions;
      const nextAgentStates = hasAgent
        ? (() => {
            const cleaned = { ...current.agentStates };
            delete cleaned[event.terminal];
            return cleaned;
          })()
        : current.agentStates;
      return { sessions: nextSessions, agentStates: nextAgentStates, lastSeq: nextSeq };
    }

    case "lost": {
      const prev = current.sessions[event.terminal];
      const hasAgent = event.terminal in current.agentStates;
      if (!prev && !hasAgent) {
        return { sessions: current.sessions, agentStates: current.agentStates, lastSeq: nextSeq };
      }
      const data = parseTerminationEventData(event.data);
      const nextSessions = prev
        ? {
            ...current.sessions,
            [event.terminal]: {
              ...prev,
              state: "lost",
              reason: data.reason,
              exitCode: data.exitCode,
              signal: data.signal,
              foreground: null,
              updatedAt: nowMs,
            },
          }
        : current.sessions;
      const nextAgentStates = hasAgent
        ? (() => {
            const cleaned = { ...current.agentStates };
            delete cleaned[event.terminal];
            return cleaned;
          })()
        : current.agentStates;
      return { sessions: nextSessions, agentStates: nextAgentStates, lastSeq: nextSeq };
    }

    case "removed": {
      if (!(event.terminal in current.sessions) && !(event.terminal in current.agentStates)) {
        return { sessions: current.sessions, agentStates: current.agentStates, lastSeq: nextSeq };
      }
      const nextSessions = { ...current.sessions };
      delete nextSessions[event.terminal];
      const nextAgentStates = { ...current.agentStates };
      delete nextAgentStates[event.terminal];
      return { sessions: nextSessions, agentStates: nextAgentStates, lastSeq: nextSeq };
    }

    case "title": {
      const prev = current.sessions[event.terminal];
      const data = parseTitleEventData(event.data);
      if (!prev || !data || prev.title === data.title) {
        return { sessions: current.sessions, agentStates: current.agentStates, lastSeq: nextSeq };
      }
      return {
        sessions: {
          ...current.sessions,
          [event.terminal]: { ...prev, title: data.title, updatedAt: nowMs },
        },
        agentStates: current.agentStates,
        lastSeq: nextSeq,
      };
    }

    case "size": {
      const prev = current.sessions[event.terminal];
      const data = parseSizeEventData(event.data);
      if (!prev || !data || (prev.cols === data.cols && prev.rows === data.rows)) {
        return { sessions: current.sessions, agentStates: current.agentStates, lastSeq: nextSeq };
      }
      return {
        sessions: {
          ...current.sessions,
          [event.terminal]: {
            ...prev,
            cols: data.cols,
            rows: data.rows,
            updatedAt: nowMs,
          },
        },
        agentStates: current.agentStates,
        lastSeq: nextSeq,
      };
    }

    case "foreground": {
      const prev = current.sessions[event.terminal];
      if (!prev) {
        return { sessions: current.sessions, agentStates: current.agentStates, lastSeq: nextSeq };
      }
      return {
        sessions: {
          ...current.sessions,
          [event.terminal]: { ...prev, foreground: event.data, updatedAt: nowMs },
        },
        agentStates: current.agentStates,
        lastSeq: nextSeq,
      };
    }

    case "labels": {
      const prev = current.sessions[event.terminal];
      const data = parseLabelsEventData(event.data);
      if (!prev || !data) {
        return { sessions: current.sessions, agentStates: current.agentStates, lastSeq: nextSeq };
      }
      return {
        sessions: {
          ...current.sessions,
          [event.terminal]: { ...prev, labels: data.labels, updatedAt: nowMs },
        },
        agentStates: current.agentStates,
        lastSeq: nextSeq,
      };
    }

    case "status": {
      const data = parseStatusEventData(event.data);
      if (!data) {
        return { sessions: current.sessions, agentStates: current.agentStates, lastSeq: nextSeq };
      }
      const prev = current.sessions[event.terminal];
      const turn = prev?.turn ?? current.agentStates[event.terminal]?.turn ?? 0;
      const nextSessions = prev
        ? {
            ...current.sessions,
            [event.terminal]: {
              ...prev,
              rawStatus: data.status,
              updatedAt: nowMs,
            },
          }
        : current.sessions;
      const nextAgentStates = {
        ...current.agentStates,
        [event.terminal]: agentStateFromStatusRecord(event.terminal, data.status, turn, nowMs),
      };
      return { sessions: nextSessions, agentStates: nextAgentStates, lastSeq: nextSeq };
    }

    case "turn": {
      const data = parseTurnEventData(event.data);
      if (!data) {
        return { sessions: current.sessions, agentStates: current.agentStates, lastSeq: nextSeq };
      }
      const prev = current.sessions[event.terminal];
      const prevAgent = current.agentStates[event.terminal];
      const nextSessions = prev
        ? {
            ...current.sessions,
            [event.terminal]: { ...prev, turn: data.turn, updatedAt: nowMs },
          }
        : current.sessions;
      const nextAgentStates = {
        ...current.agentStates,
        [event.terminal]: {
          sessionId: event.terminal,
          status: normalizeAgentStatusKind(data.state),
          app: prevAgent?.app ?? null,
          title: prevAgent?.title ?? null,
          message: prevAgent?.message ?? null,
          progress: prevAgent?.progress ?? null,
          turn: data.turn,
          updatedAt: nowMs,
        },
      };
      return { sessions: nextSessions, agentStates: nextAgentStates, lastSeq: nextSeq };
    }
  }
}
