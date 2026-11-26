import * as v from "valibot";
import { AppError } from "../errors/appError.ts";
import type {
  DisplayFacts,
  IpcError,
  StatusRecord,
  TerminalAttachParams,
  TerminalCreateParams,
} from "../ipc/types.ts";

/**
 * Runtime validation schemas for external IPC boundaries (Valibot).
 *
 * Role separation:
 * - Compile-time IPC types live in `src/ipc/types.ts` (backed by Rust `ts-rs` bindings).
 * - Valibot schemas in `src/validation/` validate untrusted or external runtime inputs
 *   (IPC arguments/errors, CLI/config payloads, resize bounds, dynamic `CoreEvent.data`)
 *   and statically assert compatibility with `src/ipc/types.ts` so the two never drift.
 */

export const MIN_TERMINAL_COLS = 1;
export const MAX_TERMINAL_COLS = 1000;
export const MIN_TERMINAL_ROWS = 1;
export const MAX_TERMINAL_ROWS = 1000;

export const SessionIdSchema = v.pipe(v.string(), v.minLength(1), v.maxLength(256));

export const AttachIdSchema = v.pipe(
  v.number(),
  v.integer(),
  v.minValue(1),
  v.maxValue(0xffff_ffff),
);

export const TerminalColsSchema = v.pipe(
  v.number(),
  v.integer(),
  v.minValue(MIN_TERMINAL_COLS),
  v.maxValue(MAX_TERMINAL_COLS),
);

export const TerminalRowsSchema = v.pipe(
  v.number(),
  v.integer(),
  v.minValue(MIN_TERMINAL_ROWS),
  v.maxValue(MAX_TERMINAL_ROWS),
);

export const InputKindSchema = v.picklist(["user", "automatic"]);

export const DisplayFactsSchema = v.object({
  fg: v.optional(v.nullable(v.string())),
  bg: v.optional(v.nullable(v.string())),
  cursor: v.optional(v.nullable(v.string())),
  cell: v.optional(
    v.nullable(
      v.tuple([
        v.pipe(v.number(), v.integer(), v.minValue(1)),
        v.pipe(v.number(), v.integer(), v.minValue(1)),
      ]),
    ),
  ),
}) satisfies v.GenericSchema<DisplayFacts>;

export const TerminalCreateParamsSchema = v.object({
  argv: v.optional(v.nullable(v.array(v.string()))),
  line: v.optional(v.nullable(v.string())),
  cwd: v.optional(v.nullable(v.string())),
  env: v.optional(v.nullable(v.record(v.string(), v.string()))),
  name: v.optional(v.nullable(v.string())),
  labels: v.optional(v.nullable(v.record(v.string(), v.string()))),
  cols: v.optional(v.nullable(TerminalColsSchema)),
  rows: v.optional(v.nullable(TerminalRowsSchema)),
  display: v.optional(v.nullable(DisplayFactsSchema)),
  keep: v.optional(v.nullable(v.boolean())),
}) satisfies v.GenericSchema<TerminalCreateParams>;

export const TerminalAttachParamsSchema = v.object({
  sessionId: SessionIdSchema,
  cols: TerminalColsSchema,
  rows: TerminalRowsSchema,
  history: v.optional(v.nullable(v.pipe(v.number(), v.integer(), v.minValue(0)))),
  observe: v.optional(v.nullable(v.boolean())),
  resizeNotifications: v.optional(v.nullable(v.boolean())),
  display: v.optional(v.nullable(DisplayFactsSchema)),
}) satisfies v.GenericSchema<TerminalAttachParams>;

export const ResizePayloadSchema = v.object({
  sessionId: SessionIdSchema,
  cols: TerminalColsSchema,
  rows: TerminalRowsSchema,
});

export type ResizePayload = v.InferOutput<typeof ResizePayloadSchema>;

export const IpcErrorSchema = v.object({
  code: v.pipe(v.string(), v.minLength(1)),
  message: v.string(),
  hint: v.optional(v.nullable(v.string())),
}) satisfies v.GenericSchema<IpcError>;

const StatusRecordSchema = v.object({
  state: v.string(),
  kind: v.optional(v.nullable(v.string())),
  progress: v.optional(v.nullable(v.number())),
  app: v.optional(v.string()),
  title: v.optional(v.string()),
  msg: v.optional(v.string()),
}) satisfies v.GenericSchema<StatusRecord>;

const TerminationDataSchema = v.object({
  reason: v.optional(v.nullable(v.string())),
  exitCode: v.optional(v.nullable(v.number())),
  exit_code: v.optional(v.nullable(v.number())),
  signal: v.optional(v.nullable(v.string())),
  finishedMs: v.optional(v.nullable(v.number())),
  finished_ms: v.optional(v.nullable(v.number())),
});

const StatusEventDataSchema = v.object({
  status: v.optional(v.nullable(StatusRecordSchema)),
  reported: v.optional(v.boolean()),
});

const TurnEventDataSchema = v.object({
  turn: v.number(),
  state: v.string(),
});

const TitleEventDataSchema = v.object({
  title: v.string(),
});

const SizeEventDataSchema = v.object({
  cols: TerminalColsSchema,
  rows: TerminalRowsSchema,
});

const LabelsEventDataSchema = v.object({
  labels: v.record(v.string(), v.string()),
});

export interface ParsedTerminationEventData {
  readonly reason: string | null;
  readonly exitCode: number | null;
  readonly signal: string | null;
  readonly finishedMs: number | null;
}

/**
 * Parses `CoreEvent` `finished` / `lost` `data: unknown`, normalizing both
 * raw `herdr-core` snake_case (`exit_code`, `finished_ms`) and camelCase keys.
 */
export function parseTerminationEventData(data: unknown): ParsedTerminationEventData {
  const res = v.safeParse(TerminationDataSchema, data);
  if (!res.success) {
    return { reason: null, exitCode: null, signal: null, finishedMs: null };
  }
  return {
    reason: res.output.reason ?? null,
    exitCode: res.output.exitCode ?? res.output.exit_code ?? null,
    signal: res.output.signal ?? null,
    finishedMs: res.output.finishedMs ?? res.output.finished_ms ?? null,
  };
}

export function parseStatusEventData(
  data: unknown,
): { readonly status: StatusRecord | null; readonly reported: boolean } | null {
  const res = v.safeParse(StatusEventDataSchema, data);
  if (!res.success) return null;
  return {
    status: res.output.status ?? null,
    reported: res.output.reported ?? false,
  };
}

export function parseTurnEventData(
  data: unknown,
): { readonly turn: number; readonly state: string } | null {
  const res = v.safeParse(TurnEventDataSchema, data);
  return res.success ? res.output : null;
}

export function parseTitleEventData(data: unknown): { readonly title: string } | null {
  const res = v.safeParse(TitleEventDataSchema, data);
  return res.success ? res.output : null;
}

export function parseSizeEventData(
  data: unknown,
): { readonly cols: number; readonly rows: number } | null {
  const res = v.safeParse(SizeEventDataSchema, data);
  return res.success ? res.output : null;
}

export function parseLabelsEventData(
  data: unknown,
): { readonly labels: Record<string, string> } | null {
  const res = v.safeParse(LabelsEventDataSchema, data);
  return res.success ? res.output : null;
}

export function validateResizePayload(input: unknown): ResizePayload {
  return v.parse(ResizePayloadSchema, input);
}

/**
 * Parses an unknown value against a Valibot schema and throws a structured `AppError`
 * (`code: "VALIDATION_FAILED"`) on failure instead of leaking raw Valibot errors.
 */
export function parseExternalInput<TSchema extends v.BaseSchema<unknown, unknown, v.BaseIssue<unknown>>>(
  schema: TSchema,
  input: unknown,
  context: string,
): v.InferOutput<TSchema> {
  const result = v.safeParse(schema, input);
  if (result.success) {
    return result.output;
  }
  const issues = result.issues.map((issue) => ({
    path: issue.path?.map((part) => String(part.key)).join(".") || "$",
    message: issue.message,
  }));
  throw new AppError({
    code: "VALIDATION_FAILED",
    messageKey: "error.validation.failed",
    devMessage: `${context}: ${issues.map((i) => `${i.path}: ${i.message}`).join("; ")}`,
    diagnostics: { context, issues },
  });
}
