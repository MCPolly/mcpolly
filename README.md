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
- Agent registration and status tracking (starting, running, warning, error, completed, offline, paused, errored)
- Real-time activity feed with color-coded entries
- Global error feed across all agents
- Discord webhook alerts with retry logic
- Silent agent detection

### Web Dashboard
- Dark mode with system preference detection
- Dashboard summary cards (total/running/errored/offline agent counts)
- Status filter pills for quick agent filtering
- Agent detail page with status dot, activity timeline, and metadata sidebar
- Alert rules management with notification history
- Embeddings management with semantic search UI
- API key management
- 10-second HTMX polling for live updates

### Knowledge Layer (Vector Embeddings)
- Index PRD, design, and custom documents as vector embeddings
- Semantic search across all indexed content via MCP tools and web UI
- Spawn product manager and product designer agents with relevant context
- Powered by sqlite-vec (in-process) and Ollama (local, no cloud API keys needed)

## Installation

### Option 1: Build from Source

```bash
git clone https://github.com/MCPolly/mcpolly.git
cd mcpolly
cargo build --release
```

Binaries will be at:
- `target/release/mcpolly` (HTTP server)
- `target/release/mcpolly_mcp` (MCP stdio bridge)

### Option 2: Cargo Install

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

On first run, a default API key is generated and printed to stdout. You can also create new keys through the web UI's Settings page.

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

```bash
claude mcp add mcpolly --url http://localhost:3000/mcp --header "Authorization: Bearer your-api-key-here"
```

### 5. Verify

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
