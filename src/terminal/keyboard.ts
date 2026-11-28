import type { BuiltinActionId } from "../actions/types.ts";

export interface KeyboardEventLike {
  readonly key: string;
  readonly code: string;
  readonly ctrlKey: boolean;
  readonly altKey: boolean;
  readonly metaKey: boolean;
  readonly shiftKey: boolean;
  readonly isComposing?: boolean;
  readonly keyCode?: number;
  readonly repeat?: boolean;
  readonly target?: EventTarget | null;
}

export interface AppShortcutMatch {
  readonly actionId: BuiltinActionId;
  readonly args: Record<string, unknown>;
}

export interface MatchAppShortcutOptions {
  readonly isMac: boolean;
  readonly isComposing: boolean;
  readonly hasSelection: boolean;
  readonly isEditableTarget?: boolean;
}

export interface TranslateMacOptionOptions {
  readonly isMac: boolean;
  readonly macOptionAsMeta: boolean;
  readonly isComposing: boolean;
  readonly kittyKeyboardFlags?: number;
}

/**
 * Detects macOS from `navigator.platform` / `userAgentData`.
 */
export function isMacPlatform(platform?: string): boolean {
  const value =
    platform ??
    (typeof navigator !== "undefined"
      ? ((navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData
          ?.platform ?? navigator.platform)
      : "");
  return /mac|darwin/i.test(value);
}

/**
 * Returns true if the keyboard event is part of an active IME composition session.
 * Covers WebKit's historical `keyCode === 229` and `"Process"` edge cases in addition
 * to `event.isComposing` and our store-tracked `isComposing` flag.
 */
export function isImeComposingEvent(
  event: Pick<KeyboardEventLike, "isComposing" | "keyCode" | "key">,
  storeIsComposing: boolean,
): boolean {
  return (
    storeIsComposing ||
    Boolean(event.isComposing) ||
    event.keyCode === 229 ||
    event.key === "Process"
  );
}

function resolveShortcutKey(event: Pick<KeyboardEventLike, "key" | "code">): string {
  const lowered = event.key.toLowerCase();
  if (/^[a-z0-9=+-]$/.test(lowered)) {
    return lowered;
  }
  const codeMatch = /^Key([A-Z])$/.exec(event.code);
  if (codeMatch) {
    return codeMatch[1]!.toLowerCase();
  }
  return lowered;
}

/**
 * Matches application-level keyboard shortcuts that should dispatch through
 * `ActionRegistry` rather than reaching the terminal PTY.
 *
 * Guarantees:
 * - Never matches during IME composition.
 * - Never hijacks terminal control sequences (`Ctrl+C`, `Ctrl+D`, `Ctrl+Z`, `Ctrl+A`,
 *   `Ctrl+E`, `Ctrl+R`, `Ctrl+W`, `Ctrl+[`, `Escape`, `Tab`, `Alt/Option+keys`).
 * - Leaves `Cmd+V` / `Ctrl+Shift+V` to native paste flow when appropriate so
 *   WKWebView delivers `clipboardData` directly to `@wterm/dom`'s bracketed paste handler.
 */
export function matchAppShortcut(
  event: KeyboardEventLike,
  options: MatchAppShortcutOptions,
): AppShortcutMatch | null {
  if (isImeComposingEvent(event, options.isComposing)) {
    return null;
  }

  const key = resolveShortcutKey(event);

  if (options.isMac) {
    // macOS application shortcuts use Command (`metaKey`) without Control or Option.
    if (!event.metaKey || event.ctrlKey || event.altKey) {
      return null;
    }

    if (event.shiftKey) {
      // Tauri's native macOS menu reserves Command+W for closing the window.
      if (key === "w") {
        return { actionId: "terminal.close", args: {} };
      }
      if (key === "d") {
        return { actionId: "app.toggleDiagnostics", args: {} };
      }
      if (key === "+" || key === "=" || event.code === "Equal") {
        return { actionId: "terminal.zoomIn", args: {} };
      }
      return null;
    }

    switch (key) {
      case "c":
        return options.hasSelection ? { actionId: "terminal.copySelection", args: {} } : null;
      case "a":
        return options.isEditableTarget ? null : { actionId: "terminal.selectAll", args: {} };
      case "t":
        return { actionId: "terminal.create", args: { activate: true } };
      case "=":
      case "+":
        return { actionId: "terminal.zoomIn", args: {} };
      case "-":
        return { actionId: "terminal.zoomOut", args: {} };
      case "0":
        return { actionId: "terminal.resetFontSize", args: {} };
      default:
        return null;
    }
  }

  // Non-macOS (Linux / Wayland): Ctrl+Shift+<letter> for app/terminal actions so plain Ctrl+<letter> reaches PTY.
  if (event.ctrlKey && !event.altKey && !event.metaKey) {
    if (event.shiftKey) {
      switch (key) {
        case "c":
          return options.hasSelection ? { actionId: "terminal.copySelection", args: {} } : null;
        case "a":
          return options.isEditableTarget ? null : { actionId: "terminal.selectAll", args: {} };
        case "t":
          return { actionId: "terminal.create", args: { activate: true } };
        case "w":
          return { actionId: "terminal.close", args: {} };
        case "d":
          return { actionId: "app.toggleDiagnostics", args: {} };
        case "+":
        case "=":
          return { actionId: "terminal.zoomIn", args: {} };
        default:
          return null;
      }
    }
    if (key === "=" || key === "+") {
      return { actionId: "terminal.zoomIn", args: {} };
    }
    if (key === "-") {
      return { actionId: "terminal.zoomOut", args: {} };
    }
    if (key === "0") {
      return { actionId: "terminal.resetFontSize", args: {} };
    }
  }

  return null;
}

const CODE_TO_ASCII: Readonly<Record<string, readonly [normal: string, shifted: string]>> = {
  Digit0: ["0", ")"],
  Digit1: ["1", "!"],
  Digit2: ["2", "@"],
  Digit3: ["3", "#"],
  Digit4: ["4", "$"],
  Digit5: ["5", "%"],
  Digit6: ["6", "^"],
  Digit7: ["7", "&"],
  Digit8: ["8", "*"],
  Digit9: ["9", "("],
  Minus: ["-", "_"],
  Equal: ["=", "+"],
  BracketLeft: ["[", "{"],
  BracketRight: ["]", "}"],
  Backslash: ["\\", "|"],
  Semicolon: [";", ":"],
  Quote: ["'", '"'],
  Backquote: ["`", "~"],
  Comma: [",", "<"],
  Period: [".", ">"],
  Slash: ["/", "?"],
};

/**
 * Translates macOS `Option+<physical key>` combinations into standard terminal
 * `ESC + <ascii>` Meta sequences so fish, Neovim, readline, and Codex receive
 * `\x1bb`, `\x1bf`, `\x1bd`, `\x1b.`, `\x1b\x7f`, etc. instead of macOS dead-key
 * / composed characters (`∫`, `ƒ`, `∂`, `≥`).
 *
 * Returns `null` when the key should be left to `@wterm/dom` (arrows, Kitty mode,
 * non-macOS, IME composing, or `macOptionAsMeta === false`).
 */
export function translateMacOptionKey(
  event: KeyboardEventLike,
  options: TranslateMacOptionOptions,
): string | null {
  if (!options.isMac || !options.macOptionAsMeta) {
    return null;
  }
  if (isImeComposingEvent(event, options.isComposing)) {
    return null;
  }
  if (!event.altKey || event.ctrlKey || event.metaKey) {
    return null;
  }
  // When Kitty keyboard protocol is active, let wterm's Kitty encoder handle it.
  if ((options.kittyKeyboardFlags ?? 0) !== 0) {
    return null;
  }

  if (event.code === "Backspace" || event.key === "Backspace") {
    return "\x1b\x7f";
  }

  const letterMatch = /^Key([A-Z])$/.exec(event.code);
  if (letterMatch) {
    const ch = event.shiftKey ? letterMatch[1]! : letterMatch[1]!.toLowerCase();
    return `\x1b${ch}`;
  }

  const mapped = CODE_TO_ASCII[event.code];
  if (mapped) {
    const ch = event.shiftKey ? mapped[1] : mapped[0];
    return `\x1b${ch}`;
  }

  return null;
}
