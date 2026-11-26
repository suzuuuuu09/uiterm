import type { TerminalBackendId } from "../validation/settingsSchemas.ts";
import type {
  TerminalAdapter,
  TerminalAdapterCallbacks,
  TerminalAdapterOptions,
} from "./types.ts";
import { WTermGhosttyAdapter } from "./wtermAdapter.ts";

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
    default: {
      const _exhaustive: never = backendId;
      throw new Error(`Unsupported backend: ${String(_exhaustive)}`);
    }
  }
}
