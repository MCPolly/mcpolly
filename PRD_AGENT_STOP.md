# MCPolly — Agent Stop Feature PRD

## 1. Feature Summary

Add the ability for operators to send a **stop signal** to a running agent from the MCPolly dashboard. When an operator clicks "Stop" on an agent, MCPolly records a stop request in the database and exposes it via a polling endpoint and MCP tool. The target agent detects the stop signal on its next status check and shuts itself down gracefully, transitioning to a `stopped` state.

This is a **cooperative stop** — MCPolly cannot forcibly kill an external agent process. Instead, it sets a flag that the agent is expected to poll for and honor. This design is consistent with MCPolly's architecture: agents are external processes that communicate via MCP tools over HTTP.

---

## 2. Motivation

### Problems

- **No remote control**: Operators can observe agents but cannot intervene. If an agent is misbehaving, looping, or consuming resources, the only option is to find and kill the process manually.
- **Runaway agents**: An agent stuck in a loop posting thousands of status updates has no built-in circuit breaker. The operator must SSH into the host or find the terminal to kill it.
- **Stale "running" agents**: Agents that crash without posting a final status remain in `running` state indefinitely. While `agent_silent` alerts help detect this, operators still can't mark the agent as stopped from the dashboard.
- **Multi-agent coordination**: In orchestrated workflows, an orchestrator agent may need to signal downstream agents to stop — currently impossible through MCPolly.

### Opportunity

By adding a stop signal mechanism, MCPolly moves from **observe-only** to **observe-and-control**, which is the natural next step for an agent operations platform. The cooperative polling approach keeps the architecture simple and avoids the complexity of persistent connections or push notifications.

---

## 3. User Personas

### Operator

- **Who**: A team lead or DevOps engineer monitoring agents on the MCPolly dashboard.
- **Goal**: Stop a running agent directly from the web UI without needing terminal access.
- **Pain**: Currently must find the agent's process manually. No centralized stop mechanism.
- **Key need**: A "Stop" button on the agent detail page that sends a stop signal the agent will honor.

### Agent Developer

- **Who**: A developer building agents that integrate with MCPolly.
- **Goal**: Make agents responsive to stop signals so operators can control them remotely.
- **Pain**: No standard mechanism for agents to check if they should stop. Each developer invents ad hoc solutions.
- **Key need**: A simple MCP tool (`check_stop_signal`) that returns whether a stop has been requested, and clear documentation on how to integrate it.

### Orchestrator Agent

- **Who**: An AI agent coordinating multiple downstream agents.
- **Goal**: Programmatically stop a downstream agent that is no longer needed or is misbehaving.
- **Pain**: No MCP tool exists to request an agent stop. Must rely on human intervention.
- **Key need**: A `stop_agent` MCP tool that requests a stop for a given agent ID.

---

## 4. Architecture

### 4.1 Design Approach: Cooperative Polling

MCPolly uses a **cooperative stop** model:

1. **Operator/orchestrator** sends a stop request (via UI button, API, or MCP tool).
2. MCPolly records the stop request in the `stop_requests` table with a `pending` status.
3. The agent's status update cycle includes a call to `check_stop_signal` (MCP tool) or the agent reads the stop flag from the `post_status` response.
4. When the agent detects the stop signal, it performs graceful shutdown and posts a final `stopped` status.
5. If the agent does not respond within a configurable timeout, the stop request is marked `expired` and the agent's state is force-set to `stopped` by a background task.

### 4.2 Why Not Push?

- MCPolly agents connect via HTTP request/response (MCP over HTTP). There is no persistent connection to push messages through.
- SSE/WebSocket would require agents to maintain a listener, adding complexity to every agent integration.
- Polling is consistent with the existing architecture (agents already call `post_status` periodically).
- The stop signal is embedded in the `post_status` response, so agents get it for free without any additional polling.

### 4.3 Data Flow

```
Operator clicks "Stop" on dashboard
    │
    ▼
POST /api/v1/agents/:id/stop
    │
    ▼
Insert into stop_requests (status: pending)
Update agent current_state to "stopping"
    │
    ▼
Agent calls post_status or check_stop_signal
    │
    ▼
Response includes { "stop_requested": true }
    │
    ▼
Agent performs graceful shutdown
Agent calls post_status with state "stopped"
    │
    ▼
stop_request status updated to "acknowledged"
```

### 4.4 Timeout Flow

```
Background task runs every 60 seconds
    │
    ▼
Find stop_requests with status "pending" older than timeout (default 5 min)
    │
    ▼
Mark stop_request as "expired"
Force-set agent state to "stopped"
Post status_update: "Agent did not respond to stop signal within timeout"
```

---

## 5. Database Schema

### 5.1 New Table

```sql
CREATE TABLE IF NOT EXISTS stop_requests (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id),
    requested_by TEXT NOT NULL DEFAULT 'operator',  -- 'operator', 'orchestrator', 'alert'
    reason TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',          -- 'pending', 'acknowledged', 'expired', 'cancelled'
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    resolved_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_stop_requests_agent_id ON stop_requests(agent_id);
CREATE INDEX IF NOT EXISTS idx_stop_requests_status ON stop_requests(status);
```

### 5.2 New Agent State

Add `stopped` and `stopping` to the valid states list in `models.rs`:

```rust
pub const VALID_STATES: &[&str] = &[
    "starting", "running", "warning", "error", "completed",
    "offline", "paused", "errored", "stopping", "stopped",
];
```

- **`stopping`**: Transitional state set when a stop is requested but not yet acknowledged by the agent.
- **`stopped`**: Final state set when the agent has acknowledged the stop or the timeout has expired.

---

## 6. Core Capabilities

### 6.1 Stop Request from Dashboard

Operators can stop a running agent from the web UI.

**Acceptance Criteria:**

- The agent detail page shows a "Stop Agent" button when the agent is in `starting`, `running`, `warning`, or `paused` state.
- The button is **not** shown when the agent is already `stopped`, `stopping`, `completed`, `offline`, or `errored`.
- Clicking the button sends `POST /agents/:id/stop` (HTMX) with an optional reason field.
- On success, the agent's state changes to `stopping` and the button is replaced with a "Stopping..." indicator.
- A confirmation prompt is shown before sending the stop request (inline HTMX confirm or `hx-confirm`).
- The stop request appears in the agent's activity feed.

### 6.2 Stop Request via JSON API

External consumers and orchestrators can request a stop via the REST API.

**Acceptance Criteria:**

- `POST /api/v1/agents/:id/stop` accepts an optional JSON body: `{ "reason": "string", "requested_by": "string" }`.
- Returns `200` with `{ "stop_request_id": "...", "status": "pending" }`.
- Returns `404` if the agent does not exist.
- Returns `409` if the agent already has a pending stop request or is already `stopped`/`stopping`.
- The agent's `current_state` is updated to `stopping`.
- A `status_update` record is inserted with state `stopping` and the reason as the message.

### 6.3 Stop Request via MCP Tool

Agents and orchestrators can request a stop through the MCP interface.

**Acceptance Criteria:**

- A new MCP tool `stop_agent` accepts `agent_id: String` and `reason: Option<String>`.
- Behavior mirrors the JSON API endpoint (6.2).
- Returns `{ "stop_request_id": "...", "status": "pending" }` on success.
- Returns an error if the agent is not in a stoppable state.

### 6.4 Stop Signal Detection in `post_status` Response

Agents automatically detect the stop signal without additional polling.

**Acceptance Criteria:**

- The `post_status` MCP tool response includes a `stop_requested: bool` field.
- When a pending stop request exists for the agent, `stop_requested` is `true`.
- The `post_status` JSON API response (`POST /api/v1/agents/:id/status`) also includes `stop_requested`.
- This is the **primary** mechanism for agents to detect stop signals — no additional tool call needed.

### 6.5 Explicit Stop Signal Check (MCP Tool)

For agents that need to check for stop signals outside of `post_status` calls.

**Acceptance Criteria:**

- A new MCP tool `check_stop_signal` accepts `agent_id: String`.
- Returns `{ "stop_requested": true, "reason": "...", "requested_at": "..." }` if a pending stop request exists.
- Returns `{ "stop_requested": false }` if no pending stop request exists.
- Does **not** change any state — it is a read-only check.

### 6.6 Stop Acknowledgement

When an agent detects a stop signal and shuts down, the stop request is resolved.

**Acceptance Criteria:**

- When an agent posts status with state `stopped`, MCPolly checks for a pending stop request for that agent.
- If found, the stop request's status is updated to `acknowledged` and `resolved_at` is set.
- The agent's `current_state` is set to `stopped`.
- A status update is recorded: "Agent stopped (acknowledged stop signal)".

### 6.7 Stop Request Timeout

A background task handles agents that don't respond to stop signals.

**Acceptance Criteria:**

- A background task (integrated into the existing silent-agent detection loop) runs every 60 seconds.
- It finds all `stop_requests` with `status = 'pending'` where `created_at` is older than the configurable timeout (env var `STOP_TIMEOUT_SECONDS`, default 300 seconds / 5 minutes).
- For each expired request:
  - Set `stop_requests.status` to `expired` and `resolved_at` to now.
  - Set the agent's `current_state` to `stopped`.
  - Insert a `status_update`: "Stop request expired — agent did not acknowledge within timeout".
- The timeout is configurable but should be generous enough that agents checking every 10–30 seconds will catch the signal.

### 6.8 Cancel Stop Request

Operators can cancel a pending stop request before the agent acknowledges it.

**Acceptance Criteria:**

- `DELETE /api/v1/agents/:id/stop` cancels the pending stop request.
- The stop request's status is set to `cancelled` and `resolved_at` is set.
- The agent's `current_state` is reverted to its previous state (stored in the `status_updates` table — query the most recent non-`stopping` state).
- The dashboard shows a "Cancel Stop" button when the agent is in `stopping` state.
- Returns `404` if no pending stop request exists.

---

## 7. MCP Tool Definitions

| Tool | Parameters | Returns |
|------|-----------|---------|
| `stop_agent` | `agent_id: String`, `reason?: String` | `{ stop_request_id, status }` |
| `check_stop_signal` | `agent_id: String` | `{ stop_requested: bool, reason?, requested_at? }` |

Updated existing tool:

| Tool | Change |
|------|--------|
| `post_status` | Response now includes `stop_requested: bool` |

---

## 8. HTTP API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1/agents/:id/stop` | Request agent stop (body: `{ reason?, requested_by? }`) |
| `DELETE` | `/api/v1/agents/:id/stop` | Cancel pending stop request |
| `GET` | `/api/v1/agents/:id/stop` | Get current stop request status |

All endpoints require API key authentication.

---

## 9. UI Changes

### 9.1 Agent Detail Page

- **Stop button**: Red "Stop Agent" button in the sidebar card, below the "Set Status" section. Only visible when agent is in a stoppable state (`starting`, `running`, `warning`, `paused`).
- **Confirmation**: `hx-confirm="Are you sure you want to stop this agent?"` on the button.
- **Stopping indicator**: When agent is in `stopping` state, show a yellow "Stopping..." badge and a "Cancel Stop" button instead of the stop button.
- **Reason field**: Optional text input above the stop button for providing a reason.

### 9.2 Dashboard

- **Status dot**: `stopping` state gets a yellow/amber dot (same as `warning`).
- **Status badge**: `stopped` state gets a grey badge. `stopping` gets a yellow badge.
- **Filter pills**: Add `stopping` and `stopped` to the status filter options.

### 9.3 Activity Feed

- Stop-related status updates (`stopping`, `stopped`) appear in the activity feed with a distinct visual treatment (e.g., red-orange left border for `stopping`, grey for `stopped`).

---

## 10. Agent Integration Guide

### 10.1 Zero-Effort Integration (Recommended)

Agents that already call `post_status` periodically get stop signal detection for free. The response now includes `stop_requested`:

```json
{ "status": "ok", "stop_requested": true }
```

Agent integration instructions (in `.cursor/rules/mcpolly.mdc` and `CLAUDE.md`) should be updated to include:

> After every `post_status` call, check the `stop_requested` field in the response. If `true`, perform a graceful shutdown: save any work in progress, call `post_status` with state `stopped` and a message describing the shutdown, then exit.

### 10.2 Explicit Polling

For agents that need to check between status updates:

```
Tool: check_stop_signal
Input: { "agent_id": "<your-agent-id>" }
Output: { "stop_requested": true, "reason": "Operator requested stop", "requested_at": "2026-03-08 14:30:00" }
```

### 10.3 Graceful Shutdown Pattern

Recommended agent behavior on receiving a stop signal:

1. Complete any in-progress atomic operation (don't stop mid-write).
2. Save or commit partial work if possible.
3. Call `post_status` with state `stopped` and a message: "Stopped by operator: {reason}".
4. Exit.

---

## 11. Implementation Plan

### Phase 1: Database and Models

1. Add `stop_requests` table to `db.rs` migrations.
2. Add `stopping` and `stopped` to `VALID_STATES` in `models.rs`.
3. Add `StopRequest` model struct.

### Phase 2: API Endpoints

4. Add `POST /api/v1/agents/:id/stop` endpoint in `api.rs`.
5. Add `DELETE /api/v1/agents/:id/stop` endpoint in `api.rs`.
6. Add `GET /api/v1/agents/:id/stop` endpoint in `api.rs`.
7. Modify `post_status` response (both MCP and API) to include `stop_requested`.

### Phase 3: MCP Tools

8. Add `stop_agent` tool to `mcp_server.rs`.
9. Add `check_stop_signal` tool to `mcp_server.rs`.
10. Update `post_status` tool to include `stop_requested` in response.

### Phase 4: Background Task

11. Add stop request timeout logic to the existing silent-agent background task in `alerts.rs`.

### Phase 5: UI

12. Add stop button and stopping indicator to `agent_detail.html`.
13. Add `stopping`/`stopped` styling to `base.html` CSS.
14. Add HTMX endpoint for the stop action in `templates.rs`.
15. Update dashboard filter pills to include new states.

### Phase 6: Documentation

16. Update agent integration instructions in `.cursor/rules/mcpolly.mdc`.
17. Update `GLOBAL_CLAUDE.md` with stop signal handling instructions.
18. Update `README.md` with stop feature documentation.

### Dependency Order

```
Phase 1: Schema + Models
    │
    ├──▶ Phase 2: API Endpoints
    │        │
    │        ├──▶ Phase 3: MCP Tools
    │        │
    │        └──▶ Phase 5: UI
    │
    └──▶ Phase 4: Background Task

Phase 6: Documentation (after all phases)
```

---

## 12. Configuration

| Env Var | Default | Description |
|---------|---------|-------------|
| `STOP_TIMEOUT_SECONDS` | `300` | Seconds to wait for agent to acknowledge stop before force-setting to stopped |

---

## 13. Error Handling

| Scenario | Behavior |
|----------|----------|
| Stop requested for non-existent agent | Return `404 Not Found` |
| Stop requested for already stopping agent | Return `409 Conflict`: "Agent already has a pending stop request" |
| Stop requested for stopped/completed/offline agent | Return `409 Conflict`: "Agent is not in a stoppable state" |
| Agent posts status after stop timeout | Accept the status normally; the expired stop request is already resolved |
| Cancel stop for agent with no pending request | Return `404 Not Found` |
| Multiple rapid stop requests | Only one pending stop request per agent at a time; reject duplicates with `409` |

---

## 14. Non-Goals / Out of Scope

- **Forceful process termination**: MCPolly cannot kill agent processes. This is a cooperative signal only.
- **Stop signal via SSE/WebSocket push**: Agents must poll or check the `post_status` response. Push delivery is a future enhancement.
- **Batch stop (stop all agents)**: Stop one agent at a time. Batch operations are a future enhancement.
- **Stop signal encryption or signing**: Stop requests are authenticated via the same API key mechanism. No additional signing.
- **Agent restart**: This PRD covers stop only. Restart (stop + re-register + re-run) is a separate feature.
- **Stop signal priority levels**: No urgency levels (e.g., graceful vs. immediate). All stops are graceful.
- **Stop hooks or callbacks**: No webhook notification on stop. Operators can configure alert rules for the `stopping`/`stopped` state transitions if needed.

---

## 15. Success Criteria

- An operator can click "Stop Agent" on the dashboard, and an agent that polls every 10 seconds stops within 20 seconds.
- The `post_status` response includes `stop_requested: true` with less than 1ms additional query overhead.
- The stop timeout background task correctly force-stops agents that don't respond within the configured timeout.
- An orchestrator agent can call `stop_agent` via MCP and the target agent stops on its next `post_status` call.
- Cancelling a stop request before the agent acknowledges it correctly reverts the agent to its previous state.
- The feature adds no new external dependencies.

---

## 16. Future Enhancements

- **Stop via alert rule**: Add a new alert action `stop_agent` that automatically stops an agent when a condition is met (e.g., stop agent if it has been erroring for 5 minutes).
- **Batch stop**: "Stop All" button on the dashboard or API endpoint to stop multiple agents at once.
- **Restart**: Combine stop with a re-registration and initial status to restart an agent.
- **SSE push for stop signals**: If SSE is added (enhancement E-12), stop signals could be pushed instantly instead of polled.
- **Stop signal acknowledgement with metadata**: Allow agents to include a reason or summary of saved work when acknowledging a stop.
- **Stop history page**: A dedicated page showing all stop requests, their outcomes, and timing.

---

*This PRD is an additive feature specification for MCPolly. It builds on the existing agent management capabilities defined in PRD.md and introduces the first agent control primitive beyond observation.*
