import type { TerminalBackendId } from "../transport/schemas.ts";
import { GespenstAdapter } from "./gespenstAdapter.ts";
import { GhosttyWebAdapter } from "./ghosttyWebAdapter.ts";
import type {
  TerminalAdapter,
  TerminalAdapterCallbacks,
  TerminalAdapterOptions,
} from "./types.ts";
import { WTermGhosttyAdapter } from "./wtermAdapter.ts";
import { XtermAdapter } from "./xtermAdapter.ts";

export function createTerminalAdapter(
  backendId: TerminalBackendId,
  callbacks: TerminalAdapterCallbacks,
  options: TerminalAdapterOptions = {},
): TerminalAdapter {
  switch (backendId) {
    case "wterm-ghostty-dom":
      return new WTermGhosttyAdapter("dom", callbacks, options);
    case "wterm-ghostty-webgl":
      return new WTermGhosttyAdapter("webgl", callbacks, options);
    case "gespenst":
      return new GespenstAdapter(callbacks, options);
    case "ghostty-web":
      return new GhosttyWebAdapter(callbacks, options);
    case "xterm":
      return new XtermAdapter(callbacks, options);
    default: {
      const _exhaustive: never = backendId;
      throw new Error(`Unsupported backend: ${String(_exhaustive)}`);
    }
  }
}
