# MCPolly

Agent-native status, observability, and knowledge platform for AI agents. MCPolly is an MCP server that lets AI agents report their progress, errors, and state in real time — and gives humans a unified web dashboard to monitor everything. Agents can also index and semantically search product documents to stay aligned.

Built with Rust, Axum, SQLite, and HTMX. Designed to self-host on minimal hardware.

## Architecture

```
┌─────────────────┐   MCP Streamable HTTP   ┌──────────────────┐
│  AI Agent       │◄───────────────────────►│  MCPolly Server  │
│  (Cursor, etc.) │   JSON-RPC / SSE        │  (Axum + SQLite) │
└─────────────────┘   Bearer auth           └──────┬───────────┘
                                                   │
                                          ┌────────▼────────┐
                                          │   Web Dashboard  │
                                          │   (HTMX UI)      │
                                          └─────────────────┘
                                                   │
                                          ┌────────▼────────┐
                                          │  Ollama (local)  │
                                          │  all-MiniLM      │
                                          └─────────────────┘
```

MCPolly exposes a `/mcp` endpoint that speaks the MCP Streamable HTTP protocol (JSON-RPC over HTTP with SSE streaming). Agent platforms connect directly — no subprocess binary needed.

- **MCPolly Server** — The Axum HTTP server that stores agent data in SQLite, serves the web UI, evaluates alert rules, manages vector embeddings, and hosts the MCP endpoint.
- **Ollama** — Local LLM server used for generating vector embeddings with the all-MiniLM model (384 dimensions).
- **mcpolly_mcp** *(optional)* — A lightweight stdio binary for platforms that don't support HTTP MCP transport. It bridges stdio to the MCPolly HTTP API.

## Features

### Agent Observability
- Agent registration and status tracking (starting, running, warning, error, completed, offline, paused, errored, stopping, stopped)
- Real-time activity feed with color-coded entries
- Global error feed across all agents
- Webhook alerts (Discord, Slack, generic) with retry logic
- Configurable alerts for **any status change** — error, completed, running, starting, warning, paused, stopped, offline, or a catch-all "any status" rule
- Silent agent detection (background checker)

### Web Dashboard
- Dark mode with system preference detection
- First-run setup wizard with auto-login and MCP configuration snippets
- Instance reset from the login page (revokes all keys, generates a new one, re-enters setup wizard)
- Health strip summary (total/running/errored/offline agent counts)
- Status filter pills for quick agent filtering
- Agent detail page with tabbed content (Activity, Errors, Info)
- Global command bar (Cmd+K) searching agents, errors, and knowledge
- Alert rules management with notification history
- Knowledge page with semantic search UI
- API key management
- Settings page with server info and session management
- 10-second HTMX polling for live updates

### Knowledge Layer (Vector Embeddings)
- Index PRD, design, and custom documents as vector embeddings
- Semantic search across all indexed content via MCP tools and web UI
- Spawn product manager and product designer agents with relevant context
- Powered by sqlite-vec (in-process) and Ollama (local, no cloud API keys needed)

## Installation

### Option 1: Install Script (recommended)

Detects your OS and architecture, downloads the correct binaries from GitHub Releases:

```bash
curl -fsSL https://raw.githubusercontent.com/MCPolly/mcpolly/main/install.sh | bash
```

Customize with environment variables:

```bash
# Install only the MCP stdio bridge
MCPOLLY_BINARY=mcp curl -fsSL https://raw.githubusercontent.com/MCPolly/mcpolly/main/install.sh | bash

# Install to a custom directory
MCPOLLY_INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/MCPolly/mcpolly/main/install.sh | bash

# Install a specific version
MCPOLLY_VERSION=v0.2.0 curl -fsSL https://raw.githubusercontent.com/MCPolly/mcpolly/main/install.sh | bash
```

Supported platforms: Linux (x86_64, aarch64, armv7), macOS (x86_64, Apple Silicon), Windows (x86_64).

### Option 2: Build from Source

```bash
git clone https://github.com/MCPolly/mcpolly.git
cd mcpolly
cargo build --release
```

Binaries will be at:
- `target/release/mcpolly` (HTTP server)
- `target/release/mcpolly_mcp` (MCP stdio bridge)

### Option 3: Cargo Install

```bash
cargo install mcpolly --bin mcpolly_mcp
```

## Setup

### 1. Install Ollama (for vector embeddings)

```bash
# Install Ollama (https://ollama.ai)
curl -fsSL https://ollama.ai/install.sh | sh

# Pull the embedding model
ollama pull all-minilm

# Ollama runs on http://localhost:11434 by default
```

Vector embeddings are optional — MCPolly works without Ollama, but semantic search and agent spawning with context will be unavailable.

### 2. Start the MCPolly Server

```bash
PORT=3000 RUST_LOG=mcpolly=info ./mcpolly
```

The SQLite database (`mcpolly.db`) is created automatically on first run. The web dashboard is available at `http://localhost:3000`.

### 3. Get Your API Key

On first run, MCPolly generates a default API key and opens the **setup wizard** in your browser. The wizard displays the key and provides ready-to-copy MCP configuration snippets. The key is also printed to the server terminal:

```
╔═══════════════════════════════════════════════════════════════╗
║  DEFAULT API KEY (save this — it will not be shown again!)   ║
║                                                               ║
║  mcp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx          ║
║                                                               ║
╚═══════════════════════════════════════════════════════════════╝
```

You can create additional keys through the web UI's Settings page (`http://localhost:3000/settings`).

**Forgot your API key?** Click "Forgot API key? Reset your instance" on the login page. This revokes all existing keys, generates a new one, and takes you back through the setup wizard.

### 4. Configure the MCP Client

Add MCPolly to your AI agent platform's MCP configuration.

#### Cursor (HTTP — recommended)

Edit `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "mcpolly": {
      "url": "http://localhost:3000/mcp",
      "headers": {
        "Authorization": "Bearer your-api-key-here"
      }
    }
  }
}
```

This connects directly to the MCPolly server's MCP endpoint over HTTP. No binary to install.

#### Cursor (stdio — alternative)

If your platform doesn't support HTTP MCP transport, use the stdio binary:

```json
{
  "mcpServers": {
    "mcpolly": {
      "command": "/path/to/mcpolly_mcp",
      "env": {
        "MCPOLLY_URL": "http://localhost:3000",
        "MCPOLLY_API_KEY": "your-api-key-here"
      }
    }
  }
}
```

#### Claude Code

MCPolly connects to Claude Code via HTTP transport. There are two ways to configure it.

**Option A: CLI (user-level, available in all projects)**

```bash
claude mcp add mcpolly --transport http http://localhost:3000/mcp \
  --header "Authorization: Bearer your-api-key-here"
```

**Option B: Project-level `.mcp.json` (checked into the repo)**

Create `.mcp.json` in your project root:

```json
{
  "mcpServers": {
    "mcpolly": {
      "type": "http",
      "url": "http://localhost:3000/mcp",
      "headers": {
        "Authorization": "Bearer your-api-key-here"
      }
    }
  }
}
```

This makes MCPolly available to every Claude Code session in that project automatically — no per-user setup required.

**Auto-approve MCPolly tools (no permission prompts):**

Add `mcp__mcpolly__*` to the allow list in your project or user settings:

```bash
# Project-level (.claude/settings.json — shared with team)
# or user-level (~/.claude/settings.json — personal)
```

```json
{
  "permissions": {
    "allow": [
      "mcp__mcpolly__*"
    ]
  }
}
```

This lets Claude Code call all MCPolly tools in the background without asking for confirmation each time.

**Verify the connection:**

```bash
claude mcp list
```

MCPolly should appear with status `connected`. You can also test from within a Claude Code session:

> "Use mcpolly to list all registered agents."

**Automatic agent integration via CLAUDE.md**

To have Claude Code automatically register itself and post status updates on every task, add the following to your project's `CLAUDE.md`:

```markdown
## Agent Observability

At the start of every task:
1. Call `register_agent` with your agent name and a brief description.
2. Call `post_status` with state `starting` and a summary of the task.

As you work, call `post_status` at major milestones (state: `running`).

On completion, call `post_status` with state `completed`.

If you encounter an unrecoverable error, call `post_error` before stopping.
```

### 5. Enable MCPolly for All Agent Sessions (Global Setup)

To have every Claude Code session — across all projects — automatically register and report status to MCPolly, configure both the MCP server and agent instructions globally.

**Step 1: Add MCPolly as a global MCP server**

```bash
claude mcp add mcpolly --transport http http://localhost:3000/mcp \
  --header "Authorization: Bearer your-api-key-here"
```

This makes the MCPolly MCP tools available in every Claude Code session without per-project `.mcp.json` files.

**Step 2: Auto-approve MCPolly tools globally**

Edit `~/.claude/settings.json` (create it if it doesn't exist):

```json
{
  "permissions": {
    "allow": [
      "mcp__mcpolly__*"
    ]
  }
}
```

This prevents Claude Code from prompting for permission each time it calls a MCPolly tool.

**Step 3: Add agent instructions to your global CLAUDE.md**

Append the contents of [`GLOBAL_CLAUDE.md`](GLOBAL_CLAUDE.md) to `~/.claude/CLAUDE.md` (create it if it doesn't exist):

```bash
cat /path/to/mcpolly/GLOBAL_CLAUDE.md >> ~/.claude/CLAUDE.md
```

Or manually add:

```markdown
## Agent Observability

At the start of every task:
1. Call `register_agent` with your agent name and a brief description.
2. Call `post_status` with state `starting` and a summary of the task.

As you work, call `post_status` at major milestones (state: `running`).

On completion, call `post_status` with state `completed`.

If you encounter an unrecoverable error, call `post_error` before stopping.
```

The global `~/.claude/CLAUDE.md` file is loaded into every Claude Code session automatically. With this in place, every agent — in any project — will register itself with MCPolly, post status updates as it works, and report completion or errors.

### 6. Verify

Open a new Cursor agent session and ask it to list agents:

> "Use mcpolly to list all registered agents."

## Automatic Agent Integration

MCPolly can be configured so that every AI agent session automatically registers itself and posts status updates — no manual prompting required.

### How It Works

Copy the provided Cursor rules file into any project:

```bash
mkdir -p .cursor/rules
cp /path/to/mcpolly/.cursor/rules/mcpolly.mdc .cursor/rules/
```

This rule file instructs every Cursor agent in that project to:

1. **Register** itself with MCPolly at the start of each task
2. **Post status updates** as it works through major steps
3. **Report completion** when the task finishes
4. **Report errors** if something goes wrong

## MCP Tools

Once configured, the following MCP tools are available to AI agents:

### Agent Management

| Tool | Description |
|------|-------------|
| `register_agent` | Register a new agent (name + description). Returns an agent ID. Idempotent on name. |
| `post_status` | Post a status update (state + message). States: `starting`, `running`, `warning`, `error`, `completed`, `offline`, `paused`, `errored`. |
| `post_error` | Report an error with severity (`error`, `warning`, `critical`). Triggers configured alerts. |
| `list_agents` | List all registered agents and their current status. |
| `get_agent_activity` | Get the recent activity timeline for a specific agent. |

### Vector Embeddings

| Tool | Description |
|------|-------------|
| `update_prd_embeddings` | Index a PRD document for semantic search (chunks markdown, generates embeddings via Ollama). |
| `update_design_embeddings` | Index a design document for semantic search. |
| `search_embeddings` | Search indexed documents using natural language. Returns ranked results by similarity. |
| `list_embedding_sources` | List all indexed embedding sources with chunk counts and timestamps. |
| `delete_embeddings` | Delete all embeddings for a given source name. |

### Agent Spawning

| Tool | Description |
|------|-------------|
| `spawn_product_manager` | Spawn a product manager agent with relevant context from embeddings. |
| `spawn_product_designer` | Spawn a product designer agent with relevant PRD and design context. |

## Environment Variables

### MCPolly Server

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3000` | HTTP server port |
| `DATABASE_URL` | `mcpolly.db` | Path to SQLite database file |
| `RUST_LOG` | `info` | Log level (`trace`, `debug`, `info`, `warn`, `error`) |
| `OLLAMA_URL` | `http://localhost:11434` | Base URL for the Ollama API |
| `OLLAMA_EMBEDDING_MODEL` | `all-minilm` | Embedding model name |

### MCP Binary (`mcpolly_mcp`)

| Variable | Required | Description |
|----------|----------|-------------|
| `MCPOLLY_URL` | Yes | Base URL of the MCPolly HTTP server |
| `MCPOLLY_API_KEY` | Yes | API key for authentication |

## API Endpoints

### JSON API (`/api/v1/...`, requires API key header)

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/agents` | List all agents |
| `POST` | `/api/v1/agents/register` | Register an agent |
| `GET` | `/api/v1/agents/:id` | Get agent detail |
| `GET` | `/api/v1/agents/:id/activity` | Get agent activity |
| `GET` | `/api/v1/agents/:id/errors` | Get agent errors |
| `POST` | `/api/v1/status` | Post status update |
| `POST` | `/api/v1/errors` | Post error |
| `GET/POST` | `/api/v1/alerts` | List/create alert rules |
| `GET` | `/api/v1/alerts/history` | Alert notification history |
| `DELETE` | `/api/v1/alerts/:id` | Delete alert rule |
| `GET/POST` | `/api/v1/keys` | List/create API keys |
| `DELETE` | `/api/v1/keys/:id` | Revoke API key |
| `POST` | `/api/v1/embeddings/index` | Index a document |
| `GET` | `/api/v1/embeddings/search` | Semantic search |
| `GET` | `/api/v1/embeddings/sources` | List embedding sources |
| `DELETE` | `/api/v1/embeddings/sources/:name` | Delete embeddings |
| `GET` | `/api/v1/server/info` | Server version, uptime, DB size |

### Alert Conditions

Alert rules can be configured for any agent status change:

| Condition | Fires when |
|-----------|------------|
| `any_status` | Any status update from the agent |
| `agent_completed` | Agent posts `completed` state |
| `agent_running` | Agent posts `running` state |
| `agent_starting` | Agent posts `starting` state |
| `agent_error` | Agent posts `error` or `errored` state |
| `agent_warning` | Agent posts `warning` state |
| `agent_paused` | Agent posts `paused` state |
| `agent_stopped` | Agent posts `stopped` state |
| `agent_stopping` | Agent posts `stopping` state |
| `agent_offline` | Agent goes silent (background checker) |

Supported channels: Discord, Slack, and generic webhook (plain JSON POST).

### MCP Endpoint

| Path | Description |
|------|-------------|
| `/mcp` | MCP Streamable HTTP endpoint (JSON-RPC + SSE) |

## Development

### Local Dev Server

```bash
PORT=3000 RUST_LOG=mcpolly=info cargo run --bin mcpolly
```

### Build MCP Binary

```bash
cargo build --bin mcpolly_mcp
```

### Release Build

```bash
cargo build --release
```

## Upgrading

Use the upgrade script to safely update a running MCPolly instance:

```bash
./upgrade.sh
```

The script will:
1. Download and verify the new binary from GitHub Releases
2. Back up the SQLite database (keeps the last 5 backups)
3. Gracefully stop the service
4. Swap the binary
5. Restart and health-check — with automatic rollback if the new version fails to start

Options:

```bash
# Upgrade to a specific version
MCPOLLY_VERSION=v0.3.0 ./upgrade.sh

# Dry run (see what would happen without changing anything)
MCPOLLY_DRY_RUN=1 ./upgrade.sh

# Skip database backup
MCPOLLY_SKIP_BACKUP=1 ./upgrade.sh
```

The upgrade script auto-detects the install location, systemd service type (user or system), and database path.

## Deployment

MCPolly is designed to run on minimal hardware. A $4/month VPS is sufficient.

### Prerequisites

- Ollama installed and running with `all-minilm` model pulled (for embeddings)
- Port 3000 (or configured port) available

### systemd Service

Create `/etc/systemd/system/mcpolly.service`:

```ini
[Unit]
Description=MCPolly Agent Observability Server
After=network.target

[Service]
Type=simple
User=mcpolly
WorkingDirectory=/opt/mcpolly
ExecStart=/opt/mcpolly/mcpolly
Environment=PORT=3000
Environment=RUST_LOG=mcpolly=info
Environment=DATABASE_URL=/opt/mcpolly/data/mcpolly.db
Environment=OLLAMA_URL=http://localhost:11434
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now mcpolly
```

### Reverse Proxy (Caddy)

```
mcpolly.example.com {
    reverse_proxy localhost:3000
}
```

Caddy handles TLS automatically via Let's Encrypt.

## License

MIT
