/**
 * Minimal typed helper API for Phase 0 TypeScript config PoC.
 * Node.js 24 LTS executes .ts files directly via native type stripping.
 */

export interface ActionContext {
  callHost: (method: string, params?: Record<string, unknown>) => Promise<unknown>;
}

export interface AgentStatusEvent {
  terminal: string;
  status: string | null;
  turn: number;
}

export interface UitermConfig {
  shell: string[];
  font: {
    family: string;
    size: number;
  };
  theme: string;
  actions?: Record<string, (args: unknown, ctx: ActionContext) => unknown | Promise<unknown>>;
  hooks?: {
    onAgentStatus?: (event: AgentStatusEvent, ctx: ActionContext) => unknown | Promise<unknown>;
    onTerminalCreated?: (
      event: { terminal: string },
      ctx: ActionContext,
    ) => unknown | Promise<unknown>;
  };
}

export function defineConfig(config: UitermConfig): UitermConfig {
  return config;
}

export function createAction<TArgs = unknown, TResult = unknown>(
  fn: (args: TArgs, ctx: ActionContext) => TResult | Promise<TResult>,
): (args: unknown, ctx: ActionContext) => TResult | Promise<TResult> {
  return (args, ctx) => fn(args as TArgs, ctx);
}
