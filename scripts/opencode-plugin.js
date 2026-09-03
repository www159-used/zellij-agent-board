// OpenCode plugin → zellij-agent-board hook. Events are mapped to
// the board's Cursor-style names before the shared hook script runs.
export const ZellijAgentBoard = async () => {
  const hook = `${process.env.HOME}/.config/opencode/plugins/zellij-agent-board-hook.sh`;

  const report = (event, payload) =>
    import("node:child_process")
      .then(
        ({ spawn }) =>
          new Promise((resolve) => {
            const child = spawn(hook, [event], {
              stdio: ["pipe", "ignore", "ignore"],
            });
            child.on("error", () => resolve());
            child.on("close", () => resolve());
            try {
              child.stdin.end(JSON.stringify(payload ?? {}));
            } catch {
              resolve();
            }
          }),
      )
      .catch(() => undefined);

  return {
    event: async ({ event }) => {
      const map = {
        "session.created": "sessionStart",
        "session.deleted": "sessionEnd",
        "session.idle": "stop",
        "session.compacted": "preCompact",
      };
      const mapped = map[event?.type];
      if (mapped) {
        await report(mapped, { hook_event_name: mapped });
      }
    },
    "chat.message": async (_input, output) => {
      const prompt =
        output?.message?.content ?? output?.parts?.[0]?.text ?? "";
      await report("beforeSubmitPrompt", { prompt: String(prompt) });
    },
    // Plugin Hooks: before mutates `output.args`; after reads `input.args`.
    "tool.execute.before": async (input, output) => {
      await report("preToolUse", {
        tool_name: input?.tool,
        tool_input: output?.args ?? {},
      });
    },
    "tool.execute.after": async (input) => {
      await report("postToolUse", {
        tool_name: input?.tool,
        tool_input: input?.args ?? {},
      });
    },
    "experimental.session.compacting": async () => {
      await report("preCompact", {});
    },
  };
};
