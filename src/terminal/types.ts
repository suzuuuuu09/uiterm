import type { TerminalBackendId } from "../validation/settingsSchemas.ts";
import type {
  CursorBlinkMode,
  CursorStyle,
  TerminalAppearance,
  TerminalTheme,
} from "./appearance.ts";
import type { CellMetrics } from "./fonts.ts";

export interface TerminalAdapterCallbacks {
  /** User-originated input (keyboard, IME commit, paste, mouse). */
  onUserInput: (bytes: Uint8Array) => void;
  /** Terminal-originated auto-responses (DA1, DA2, DSR, OSC 11, etc.). */
  onAutoResponse: (bytes: Uint8Array) => void;
  /** Grid dimension changes. */
  onResize: (cols: number, rows: number) => void;
  isComposing?: () => boolean;
}

export interface TerminalAdapterOptions {
  scrollbackLines?: number;
  fontSize?: number;
  lineHeight?: number;
  fontFamily?: string;
  nerdFontFallback?: boolean;
  cursorStyle?: CursorStyle;
  cursorBlink?: CursorBlinkMode;
  theme?: TerminalTheme;
  macOptionAsMeta?: boolean;
  renderingPaused?: boolean;
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
  hasSelection?: () => boolean;
  selectAll?: () => Promise<boolean>;
  clearSelection?: () => void;
  paste?: (text: string) => void;
  applyAppearance?: (patch: Partial<TerminalAppearance> & { macOptionAsMeta?: boolean }) => void;
  getCellMetrics?: () => CellMetrics | null;
  dispose(): void;
}
