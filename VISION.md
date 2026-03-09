## MCPolly — Vision

MCPolly is the observability and status plane for AI agents: a single place where agents can report what they are doing, how it is going, and when things go wrong, and where humans can reliably see and act on that information.

### Why MCPolly exists

- **AI agents are opaque**: Today, most agents work like black boxes. They may log to console or a file, but there is no consistent, structured, user-friendly way to see their status or partial progress across tools and environments.
- **Operations needs trust and continuity**: Teams running AI-assisted workflows (coding, support, analysis, automation) need to know which agents are running, what state they are in, and whether they are stuck or failing.
- **Existing tools are not agent-native**: Traditional APM/logging tools are powerful but not tailored to the patterns of AI agents (multi-step reasoning, partial results, tool use, retries, orchestrators).

MCPolly aims to fix this by providing an agent-native status and observability layer that is easy to integrate via MCP and easy to understand for humans.

### Long-term vision

Over time, MCPolly should become:

- **The standard MCP observability backend**: Any MCP-enabled agent can plug into MCPolly out of the box to report status and partial updates.
- **A unified, cross-agent control surface**: Users see all their agents (and orchestrators) in one place, regardless of platform or vendor.
- **A living timeline of agent activity**: Every significant step (start, intermediate state, error, recovery, completion) becomes part of a structured activity feed that can be searched, filtered, and audited.
- **An intelligent alerting and insight layer**: MCPolly not only forwards alerts (via Slack/Discord/SMS/Telegram, etc.) but eventually helps detect patterns like flapping agents, degraded performance, or systemic issues across multiple agents.
- **A small, robust, self-hostable core**: Even as capabilities grow, the core remains small, resource-efficient, and suitable for self-hosting on modest hardware like a $4/month VPS.

### Product principles

- **Agent-native first**: Model the concepts that matter for AI agents (steps, tools, partial outputs, orchestrators) instead of just generic logs.
- **Human-centered operations**: Optimize views and alerts for the humans running and relying on agents; show what matters at a glance.
- **Simplicity and reliability**: Prefer a simple, predictable system over a complex, fragile one. MCPolly should be easy to run, reason about, and debug.
- **Open, interoperable, and composable**: Built on MCP and standard web primitives so it can plug into existing stacks and tooling.
- **MVP, then depth**: Start with a small, end-to-end slice (MCP ingestion → storage → UI → alerts) and deepen capabilities based on real usage.

### Vision for the MVP

The MVP is a **thin but complete vertical slice**:

- An MCP-compatible backend (Rust/Axum/Sqlite3) where agents can:
  - Authenticate via API key.
  - Post status and partial updates as they work.
  - Report errors and important events.
- A simple HTMX-based web UI where users can:
  - See a list of agents and their current/high-level status.
  - Drill into an agent to view a timeline of its recent activity and errors.
  - Configure basic alerts for key conditions and receive them via at least one or two channels (e.g. Slack/Discord).
- A deployment story that:
  - Runs comfortably on a RamNode $4/month instance.
  - Can be validated locally and then pushed to production with minimal friction.

Success for the MVP means that a small team can:

- Integrate a handful of agents with MCPolly in under an hour.
- Use the UI and alerts to gain more confidence in their agent workflows than they had with ad hoc logs or dashboards.
- Run MCPolly reliably with minimal operational overhead.

### Beyond MVP

As MCPolly evolves, we can:

- Enrich the data model with step-level traces, tool invocations, and orchestrator decisions.
- Add richer alerting, correlation, and insight features.
- Provide deeper integrations with messaging platforms, logging/metrics systems, and other observability tools.
- Offer opinionated patterns for orchestrators and frameworks to plug in with near-zero configuration.

But the core vision remains constant: **MCPolly is the small, dependable, agent-native status and observability layer that makes AI-powered systems feel understandable and trustworthy.**

