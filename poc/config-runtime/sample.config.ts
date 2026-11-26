import { createAction, defineConfig, type AgentStatusEvent } from "./api.ts";

// User-defined helper function
function formatNotification(ev: AgentStatusEvent): string {
  return `[${ev.terminal}] status=${ev.status ?? "none"} (turn ${ev.turn})`;
}

export default defineConfig({
  shell: ["fish"],
  font: {
    family: "JetBrains Mono",
    size: 13,
  },
  theme: "uiterm-dark",
  actions: {
    greet: createAction<{ name: string }, { message: string; hostReply: unknown }>(
      async (args, ctx) => {
        const hostReply = await ctx.callHost("sidebar.badge", {
          text: `Hello ${args.name}`,
        });
        return {
          message: `こんにちは、${args.name}さん`,
          hostReply,
        };
      },
    ),
  },
  hooks: {
    onAgentStatus: async (event, ctx) => {
      const label = formatNotification(event);
      const notify =
        event.status === "blocked" || event.status === "done"
          ? await ctx.callHost("notify", { body: label })
          : null;
      return { label, notified: Boolean(notify) };
    },
  },
});
