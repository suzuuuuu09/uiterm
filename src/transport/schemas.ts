import * as v from "valibot";

export const TerminalBackendIdSchema = v.picklist([
  "wterm-ghostty-dom",
  "wterm-ghostty-webgl",
  "gespenst",
  "ghostty-web",
  "xterm",
]);

export type TerminalBackendId = v.InferOutput<typeof TerminalBackendIdSchema>;

export const ResizePayloadSchema = v.object({
  sessionId: v.string(),
  cols: v.pipe(v.number(), v.integer(), v.minValue(1), v.maxValue(1000)),
  rows: v.pipe(v.number(), v.integer(), v.minValue(1), v.maxValue(1000)),
});

export type ResizePayload = v.InferOutput<typeof ResizePayloadSchema>;

export const SessionMetadataSchema = v.object({
  sessionId: v.string(),
  title: v.string(),
  cols: v.pipe(v.number(), v.integer(), v.minValue(1)),
  rows: v.pipe(v.number(), v.integer(), v.minValue(1)),
  agentState: v.optional(v.picklist(["idle", "working", "blocked", "done", "error"])),
});

export type SessionMetadata = v.InferOutput<typeof SessionMetadataSchema>;

export function validateResizePayload(input: unknown): ResizePayload {
  return v.parse(ResizePayloadSchema, input);
}
