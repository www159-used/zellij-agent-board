// Pi extension → zellij-agent-board hook. Observe only: never throw,
// never return a tool_call result. A failing tool_call handler blocks
// the tool, so this file listens to tool_execution_* instead.
export default function (pi) {
  const hook = `${process.env.HOME}/.pi/agent/extensions/zellij-agent-board-hook.sh`;
  let outcome = "completed";

  const report = (event, payload) => {
    import("node:child_process")
      .then(({ spawn }) => {
        const child = spawn(hook, [event], {
          stdio: ["pipe", "ignore", "ignore"],
        });
        child.on("error", () => undefined);
        try {
          child.stdin.end(JSON.stringify(payload ?? {}));
        } catch {
          /* observe only */
        }
      })
      .catch(() => undefined);
  };

  pi.on("session_start", () => {
    report("session_start", {});
  });

  // reload / new / resume / fork keep the process. Only quit ends the pane.
  pi.on("session_shutdown", (event) => {
    if (event?.reason === "quit") {
      report("session_shutdown", {});
    }
  });

  pi.on("before_agent_start", (event) => {
    report("before_agent_start", { prompt: String(event?.prompt ?? "") });
  });

  pi.on("tool_execution_start", (event) => {
    const args = event?.args;
    report("tool_execution_start", {
      tool_name: event?.toolName,
      tool_input: args && typeof args === "object" ? args : {},
    });
  });

  pi.on("tool_execution_end", (event) => {
    const args = event?.args;
    const failed = Boolean(event?.isError);
    report(failed ? "postToolUseFailure" : "tool_execution_end", {
      tool_name: event?.toolName,
      tool_input: args && typeof args === "object" ? args : {},
      error: failed ? "tool error" : "",
    });
  });

  pi.on("session_before_compact", () => {
    report("session_before_compact", {});
  });

  pi.on("session_compact", () => {
    report("session_compact", {});
  });

  pi.on("agent_before_settle", (event) => {
    if (event?.outcome) {
      outcome = event.outcome;
    }
  });

  // agent_settled means Pi will not continue the turn.
  pi.on("agent_settled", () => {
    const event =
      outcome === "aborted"
        ? "interrupt"
        : outcome === "error"
          ? "stopFailure"
          : "agent_end";
    report(event, { status: outcome === "aborted" ? "aborted" : "" });
    outcome = "completed";
  });
}
