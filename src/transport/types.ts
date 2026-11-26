/**
 * Frontend-side transport contract for Phase 0 PoC.
 *
 * Designed to align with herdr-core / Tauri IPC (docs/poc/interface.md once defined):
 * - Initial screen snapshot delivered as raw UTF-8/VT bytes upon attach
 * - Continuous binary output stream (Uint8Array) pushed directly to the terminal renderer
 *   WITHOUT passing through React or Zustand state
 * - User input bytes sent to backend (separated from terminal auto-responses)
 * - Resize and explicit resync operations
 */
export interface TerminalOutputChunk {
  readonly kind: "initial_screen" | "stream" | "resync";
  readonly bytes: Uint8Array;
}

export interface TerminalTransport {
  readonly sessionId: string;
  /** Subscribe to binary output chunks. Returns unsubscribe function. */
  subscribeOutput(listener: (chunk: TerminalOutputChunk) => void): () => void;
  /** Attach to the session and trigger initial screen snapshot delivery. */
  attach(cols: number, rows: number): void;
  /** Send user keyboard/paste/mouse bytes to the PTY. */
  sendInput(bytes: Uint8Array): void;
  /** Notify backend of grid resize. */
  resize(cols: number, rows: number): void;
  /** Request full screen resync from backend VT state. */
  requestResync(): void;
  /** Release transport resources. */
  dispose(): void;
}
