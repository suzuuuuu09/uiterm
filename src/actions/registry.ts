import * as v from "valibot";
import { AppError, normalizeError } from "../errors/appError.ts";
import { type Locale, t } from "../i18n/index.ts";
import { appLogger, type Logger } from "../logging/logger.ts";
import { formatValidationIssues } from "../validation/ipcSchemas.ts";
import type {
  ActionArgsOf,
  ActionAvailability,
  ActionCaller,
  ActionContext,
  ActionDefinition,
  ActionDescriptor,
  ActionId,
  ActionResult,
  ActionResultOf,
} from "./types.ts";

export interface ExecuteActionOptions {
  readonly caller?: ActionCaller;
  readonly signal?: AbortSignal;
}

export class ActionRegistry {
  private readonly actions = new Map<ActionId, ActionDefinition<ActionId, unknown, unknown>>();
  private readonly logger: Logger;

  constructor(logger: Logger = appLogger) {
    this.logger = logger;
  }

  register<TId extends ActionId, TArgs = ActionArgsOf<TId>, TResult = ActionResultOf<TId>>(
    definition: ActionDefinition<TId, TArgs, TResult>,
  ): () => void {
    if (this.actions.has(definition.id)) {
      throw new AppError({
        code: "ACTION_INVALID_ARGS",
        messageKey: "error.action.invalidArgs",
        messageParams: { actionId: definition.id },
        devMessage: `Action already registered: ${definition.id}`,
        diagnostics: { actionId: definition.id },
      });
    }
    this.actions.set(
      definition.id,
      definition as unknown as ActionDefinition<ActionId, unknown, unknown>,
    );
    return () => {
      this.unregister(definition.id);
    };
  }

  unregister(id: ActionId): boolean {
    return this.actions.delete(id);
  }

  has(id: ActionId): boolean {
    return this.actions.has(id);
  }

  list(): readonly ActionDescriptor[] {
    return [...this.actions.values()].map((def) => ({
      id: def.id,
      owner: def.owner,
      titleKey: def.titleKey,
      ...(def.descriptionKey !== undefined ? { descriptionKey: def.descriptionKey } : {}),
    }));
  }

  getLabel(id: ActionId, locale: Locale): { title: string; description?: string } | null {
    const def = this.actions.get(id);
    if (!def) return null;
    return {
      title: t(locale, def.titleKey),
      ...(def.descriptionKey !== undefined ? { description: t(locale, def.descriptionKey) } : {}),
    };
  }

  canExecute<TId extends ActionId>(
    id: TId,
    args?: ActionArgsOf<TId>,
    options: ExecuteActionOptions = {},
  ): ActionAvailability {
    const def = this.actions.get(id);
    if (!def) {
      return {
        allowed: false,
        reasonKey: "error.action.notFound",
        reasonParams: { actionId: id },
      };
    }

    const parsed = v.safeParse(def.argsSchema, args === undefined ? {} : args);
    if (!parsed.success) {
      return {
        allowed: false,
        reasonKey: "error.action.invalidArgs",
        reasonParams: { actionId: id },
      };
    }
    const parsedArgs: unknown = parsed.output;

    if (!def.canExecute) {
      return { allowed: true };
    }

    const ctx: ActionContext = {
      caller: options.caller ?? "gui",
      timestamp: Date.now(),
      signal: options.signal,
    };
    return def.canExecute(parsedArgs, ctx);
  }

  async execute<TId extends ActionId>(
    id: TId,
    args: ActionArgsOf<TId>,
    options: ExecuteActionOptions = {},
  ): Promise<ActionResult<ActionResultOf<TId>>> {
    const caller = options.caller ?? "gui";
    const def = this.actions.get(id);
    if (!def) {
      const error = new AppError({
        code: "ACTION_NOT_FOUND",
        messageKey: "error.action.notFound",
        messageParams: { actionId: id },
        devMessage: `Action not found: ${id}`,
        diagnostics: { actionId: id, caller },
      });
      this.logger.logError("actions", error);
      return { ok: false, error };
    }

    const parsed = v.safeParse(def.argsSchema, args === undefined ? {} : args);
    if (!parsed.success) {
      const issues = formatValidationIssues(parsed.issues);
      const error = new AppError({
        code: "ACTION_INVALID_ARGS",
        messageKey: "error.action.invalidArgs",
        messageParams: { actionId: id },
        devMessage: `Invalid arguments for action ${id}: ${issues.map((i) => `${i.path}: invalid (${i.type})`).join("; ")}`,
        diagnostics: { actionId: id, caller, issues },
      });
      this.logger.logError("actions", error);
      return { ok: false, error };
    }

    const ctx: ActionContext = {
      caller,
      timestamp: Date.now(),
      signal: options.signal,
    };

    try {
      if (def.canExecute) {
        const availability = def.canExecute(parsed.output, ctx);
        if (!availability.allowed) {
          const error = new AppError({
            code: "ACTION_UNAVAILABLE",
            severity: "warn",
            messageKey: availability.reasonKey,
            messageParams: availability.reasonParams,
            devMessage: `Action ${id} is unavailable (${availability.reasonKey})`,
            diagnostics: {
              actionId: id,
              caller,
              owner: def.owner,
              reasonKey: availability.reasonKey,
            },
          });
          this.logger.logError("actions", error);
          return { ok: false, error };
        }
      }

      const value = (await def.execute(parsed.output, ctx)) as ActionResultOf<TId>;
      this.logger.debug("actions", `Executed ${id}`, {
        actionId: id,
        owner: def.owner,
        caller,
      });
      return { ok: true, value };
    } catch (err) {
      const error = normalizeError(err, {
        code: "ACTION_EXECUTION_FAILED",
        messageKey: "error.action.failed",
        messageParams: { actionId: id },
        context: `action:${id}`,
        diagnostics: { actionId: id, owner: def.owner, caller },
      });
      this.logger.logError("actions", error);
      return { ok: false, error };
    }
  }
}
