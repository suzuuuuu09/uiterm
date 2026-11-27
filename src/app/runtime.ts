import type { StoreApi } from "zustand/vanilla";
import { registerBuiltinActions } from "../actions/builtin.ts";
import { ActionRegistry } from "../actions/registry.ts";
import type { UitermBackendClient } from "../ipc/types.ts";
import { appLogger, Logger } from "../logging/logger.ts";
import { pocStore } from "../poc/pocStore.ts";
import { appStore, createAppStore, type AppState } from "../state/appStore.ts";
import {
  type AdapterFactory,
  type SessionDiagnosticsSink,
  TerminalSessionManager,
} from "../terminal/sessionManager.ts";

interface AppRuntime {
  readonly store: StoreApi<AppState>;
  readonly logger: Logger;
  readonly sessionManager: TerminalSessionManager;
  readonly actions: ActionRegistry;
  setMountRoot: (host: HTMLElement | null) => void;
  getMountRoot: () => HTMLElement | null;
  dispose: () => void;
}

interface CreateAppRuntimeOptions {
  store?: StoreApi<AppState>;
  logger?: Logger;
  client?: UitermBackendClient;
  adapterFactory?: AdapterFactory;
  diagnostics?: SessionDiagnosticsSink;
  isBusy?: () => boolean;
  writeClipboard?: (text: string) => Promise<void>;
  readClipboard?: () => Promise<string>;
}

/**
 * Composition root wiring Zustand Vanilla store, Logger, TerminalSessionManager,
 * and ActionRegistry together without coupling to React.
 */
export function createAppRuntime(options: CreateAppRuntimeOptions = {}): AppRuntime {
  const store = options.store ?? createAppStore();
  const logger = options.logger ?? new Logger();
  const sessionManager = new TerminalSessionManager(options.client, options.adapterFactory, {
    store,
    logger,
    diagnostics: options.diagnostics,
  });
  const actions = new ActionRegistry(logger);

  let mountRoot: HTMLElement | null = null;

  const unregisterBuiltins = registerBuiltinActions(actions, {
    store,
    sessionManager,
    getMountRoot: () => mountRoot,
    isBusy: options.isBusy,
    writeClipboard: options.writeClipboard,
    readClipboard: options.readClipboard,
  });

  return {
    store,
    logger,
    sessionManager,
    actions,
    setMountRoot: (host) => {
      mountRoot = host;
    },
    getMountRoot: () => mountRoot,
    dispose: () => {
      unregisterBuiltins();
      sessionManager.stopEventStream();
      sessionManager.disposeAll();
      mountRoot = null;
    },
  };
}

const pocDiagnosticsSink: SessionDiagnosticsSink = {
  isEnabled: () => pocStore.getState().diagnosticsEnabled,
  addStats: (bytesDelta, repliesDelta) => pocStore.getState().addStats(bytesDelta, repliesDelta),
  pushLog: (kind, detail) => pocStore.getState().pushLog(kind, detail),
};

export const appRuntime = createAppRuntime({
  store: appStore,
  logger: appLogger,
  diagnostics: pocDiagnosticsSink,
  isBusy: () => pocStore.getState().benchRunning,
});
