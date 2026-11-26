import { Channel, invoke } from "@tauri-apps/api/core";
import {
  type AttachId,
  type ConfigEvalParams,
  type ConfigEvalResult,
  type CoreEvent,
  type CoreStatus,
  decodeOutputFrame,
  type DisplayFacts,
  type EventsSubscribeResult,
  type InputKind,
  type OutputFrame,
  type SessionId,
  type TerminalAttachParams,
  type TerminalAttachResult,
  type TerminalCreateParams,
  type TerminalInfo,
  type UitermBackendClient,
} from "./types";

const encoder = new TextEncoder();

export class TauriIpcClient implements UitermBackendClient {
  async coreStatus(): Promise<CoreStatus> {
    return invoke<CoreStatus>("core_status");
  }

  async coreReconnect(): Promise<CoreStatus> {
    return invoke<CoreStatus>("core_reconnect");
  }

  async terminalCreate(params: TerminalCreateParams = {}): Promise<TerminalInfo> {
    return invoke<TerminalInfo>("terminal_create", { params });
  }

  async terminalList(all = false): Promise<TerminalInfo[]> {
    return invoke<TerminalInfo[]>("terminal_list", { all });
  }

  async terminalAttach(
    params: TerminalAttachParams,
    onFrame: (frame: OutputFrame) => void,
  ): Promise<TerminalAttachResult> {
    const onOutput = new Channel<ArrayBuffer>();
    onOutput.onmessage = (buffer: ArrayBuffer) => {
      onFrame(decodeOutputFrame(buffer));
    };
    return invoke<TerminalAttachResult>("terminal_attach", {
      params,
      onOutput,
    });
  }

  async terminalDetach(attachId: AttachId): Promise<void> {
    await invoke<void>("terminal_detach", { attachId });
  }

  async terminalWrite(
    attachId: AttachId,
    data: Uint8Array | string,
    kind: InputKind = "user",
  ): Promise<void> {
    const bytes = typeof data === "string" ? encoder.encode(data) : data;
    await invoke<void>("terminal_write", bytes, {
      headers: {
        "x-uiterm-attach-id": String(attachId),
        "x-uiterm-input-kind": kind,
      },
    });
  }

  async terminalResize(
    attachId: AttachId,
    cols: number,
    rows: number,
  ): Promise<void> {
    await invoke<void>("terminal_resize", { attachId, cols, rows });
  }

  async terminalFocus(attachId: AttachId, focused: boolean): Promise<void> {
    await invoke<void>("terminal_focus", { attachId, focused });
  }

  async terminalFacts(
    attachId: AttachId,
    display: DisplayFacts,
  ): Promise<void> {
    await invoke<void>("terminal_facts", { attachId, display });
  }

  async terminalAck(attachId: AttachId, bytes: number): Promise<void> {
    await invoke<void>("terminal_ack", { attachId, bytes });
  }

  async terminalKill(sessionId: SessionId): Promise<void> {
    await invoke<void>("terminal_kill", { sessionId });
  }

  async eventsSubscribe(
    onEvent: (event: CoreEvent) => void,
  ): Promise<EventsSubscribeResult> {
    const channel = new Channel<{
      subscriptionId: number;
      events: CoreEvent[];
      bytes: number;
    }>();
    channel.onmessage = ({ subscriptionId, events, bytes }) => {
      try {
        for (const event of events) {
          try {
            onEvent(event);
          } catch (err) {
            console.error("Core event handler failed", err);
          }
        }
      } finally {
        if (bytes > 0) {
          void invoke("events_ack", { subscriptionId, bytes }).catch((err) => {
            console.error("Core event ACK failed", err);
          });
        }
      }
    };
    return invoke<EventsSubscribeResult>("events_subscribe", {
      onEvent: channel,
    });
  }

  async eventsUnsubscribe(subscriptionId: number): Promise<void> {
    await invoke<void>("events_unsubscribe", { subscriptionId });
  }

  async configEvalPoc(params: ConfigEvalParams): Promise<ConfigEvalResult> {
    return invoke<ConfigEvalResult>("config_eval_poc", { params });
  }
}

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
