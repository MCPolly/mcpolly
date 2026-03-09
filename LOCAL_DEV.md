## Local Development

### Default API Key

```
mcp_ixUKhIRUste0fdNvC2_77sY5-DLIKByV9MpXVJjSGSs
```

### Prerequisites

**Ollama** (required for vector embeddings):

```bash
# Install Ollama
curl -fsSL https://ollama.ai/install.sh | sh

# Pull the embedding model
ollama pull all-minilm
```

Without Ollama, MCPolly still runs but semantic search and agent spawning with context will fail gracefully.

### Run the HTTP server

```bash
PORT=3000 RUST_LOG=mcpolly=info cargo run --bin mcpolly
```

Dashboard: http://localhost:3000

### Cursor MCP integration

MCPolly exposes an MCP Streamable HTTP endpoint at `/mcp`. Configure `~/.cursor/mcp.json` to connect directly:

```json
{
  "mcpServers": {
    "mcpolly": {
      "url": "http://localhost:3000/mcp",
      "headers": {
        "Authorization": "Bearer mcp_ixUKhIRUste0fdNvC2_77sY5-DLIKByV9MpXVJjSGSs"
      }
    }
  }
}
```

No binary needed — Cursor connects to the running server over HTTP.

### Available MCP Tools

**Agent Management:**
- `register_agent` — Register an agent (idempotent on name)
- `post_status` — Post status update (starting/running/warning/error/completed/offline/paused/errored)
- `post_error` — Report an error with severity
- `list_agents` — List all agents
- `get_agent_activity` — Get agent activity timeline

**Vector Embeddings:**
- `update_prd_embeddings` — Index a PRD document
- `update_design_embeddings` — Index a design document
- `search_embeddings` — Semantic search across indexed documents
- `list_embedding_sources` — List indexed sources
- `delete_embeddings` — Delete embeddings for a source

**Agent Spawning:**
- `spawn_product_manager` — Spawn PM agent with context
- `spawn_product_designer` — Spawn designer agent with context

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3000` | HTTP server port |
| `DATABASE_URL` | `mcpolly.db` | SQLite database path |
| `RUST_LOG` | `info` | Log level |
| `OLLAMA_URL` | `http://localhost:11434` | Ollama API base URL |
| `OLLAMA_EMBEDDING_MODEL` | `all-minilm` | Embedding model name |

### UI Pages

| Route | Page |
|-------|------|
| `/` | Dashboard (summary cards, filter pills, agent table) |
| `/agents/:id` | Agent detail (activity feed, sidebar) |
| `/errors` | Global errors across all agents |
| `/alerts` | Alert rules + notification history |
| `/embeddings` | Embedding sources + semantic search |
| `/settings` | API key management |

### Dark Mode

The UI supports dark mode. Toggle via the button in the nav bar, or it auto-detects system preference. Preference is persisted in localStorage.
