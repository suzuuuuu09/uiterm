import type { TerminalBackendId } from "../transport/schemas.ts";

export interface TerminalAdapterCallbacks {
  /** User-originated input (keyboard, IME commit, paste, mouse). */
  onUserInput: (bytes: Uint8Array) => void;
  /** Terminal-originated auto-responses (DA1, DA2, DSR, OSC 11, etc.). */
  onAutoResponse: (bytes: Uint8Array) => void;
  /** Grid dimension changes. */
  onResize: (cols: number, rows: number) => void;
}

export interface TerminalAdapterOptions {
  scrollbackLines?: number;
  fontSize?: number;
  fontFamily?: string;
}

export interface TerminalAdapter {
  readonly backendId: TerminalBackendId;
  readonly cols: number;
  readonly rows: number;
  mount(container: HTMLElement): Promise<void>;
  write(bytes: Uint8Array): void;
  resize(cols: number, rows: number): void;
  fit(): void;
  focus(): void;
  setHidden(hidden: boolean): void;
  getSelection(): Promise<string>;
  dispose(): void;
}
