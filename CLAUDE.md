# MCPolly

MCPolly is an agent-native status, observability, and knowledge platform for AI agents, built as an MCP server.

## Tech Stack

- **Language**: Rust
- **Backend**: Axum server, SQLite3 database, r2d2 connection pool
- **Frontend**: HTMX with Askama server-rendered templates, dark mode support
- **Vector Search**: sqlite-vec for embeddings, Ollama (all-MiniLM) for embedding generation
- **Core**: API key-secured MCP Streamable HTTP server
- **Hosting**: RamNode $4/month instance

## Project Structure

### Source Code

- `src/main.rs` — Application entry, route wiring, server startup
- `src/api.rs` — JSON API endpoints (agents, alerts, keys, embeddings)
- `src/mcp_server.rs` — MCP tool handler (12 tools: agent management, embeddings, spawning)
- `src/mcp.rs` — HTTP API wrappers for MCP ingestion (register, status, error)
- `src/templates.rs` — HTMX UI route handlers and Askama template structs
- `src/embeddings.rs` — Vector embedding core: chunking, Ollama integration, indexing, search
- `src/alerts.rs` — Alert evaluation, Discord webhook delivery, silent agent detection
- `src/auth.rs` — API key generation, hashing, validation, middleware
- `src/db.rs` — SQLite connection pool, migrations, sqlite-vec extension registration
- `src/models.rs` — Data models, view models, state validation, relative time formatting
- `src/bin/mcpolly_mcp.rs` — Standalone MCP stdio binary for non-HTTP platforms

### Templates

- `templates/base.html` — Base layout with CSS variables, dark mode, nav bar
- `templates/dashboard.html` — Agent list with summary cards and status filter pills
- `templates/agent_detail.html` — Agent detail with activity feed and sidebar
- `templates/errors.html` — Global errors page across all agents
- `templates/alerts.html` — Alert rules + notification history
- `templates/embeddings.html` — Embedding sources and semantic search
- `templates/settings.html` — API key management
- `templates/login.html` — API key login

### Documentation

- `README.md` — Setup, installation, and configuration guide
- `IDEA.md` — Source of truth for product concept and constraints
- `VISION.md` — Long-term product vision and principles
- `PRD.md` — Product requirements document (MVP scope, user stories, features, enhancement backlog)
- `PRD_VECTOR_EMBEDDINGS.md` — Vector embeddings feature PRD
- `PRD_UI_ENHANCEMENTS.md` — UI enhancement PRD (dashboard cards, dark mode, errors page, etc.)
- `DESIGN_SPEC.md` — UX flows, screen specs, UI specification
- `DESIGN_UI_ENHANCEMENTS.md` — UI enhancement design specification with CSS/HTML patterns
- `LOCAL_DEV.md` — Local development setup and default API key
- `ORCHESTRATOR.md` — Orchestrator agent instructions
- `PRODUCT_AGENT.md` — Product planning agent instructions
- `PRODUCT_DESIGN_AGENT.md` — Product design agent instructions
- `BACKEND_AGENT.md` — Backend implementation agent instructions
- `FRONTEND_AGENT.md` — Frontend implementation agent instructions
- `GLOBAL_CLAUDE.md` — Global CLAUDE.md instructions for enabling MCPolly in all agent sessions

## UI Pages

| Route | Page | Description |
|-------|------|-------------|
| `/` | Dashboard | Summary cards (total/running/errored/offline), status filter pills, agent table |
| `/agents/:id` | Agent Detail | Status dot, activity feed with color-coded borders, metadata sidebar |
| `/errors` | Global Errors | Cross-agent error feed, paginated |
| `/alerts` | Alerts | Alert rules table + notification history section |
| `/embeddings` | Embeddings | Indexed sources list + semantic search |
| `/settings` | Settings | API key management |
| `/login` | Login | API key authentication |

## MCP Tools (13 total)

| Tool | Description |
|------|-------------|
| `register_agent` | Register an agent (idempotent on name), returns agent ID |
| `post_status` | Post status update (starting/running/warning/error/completed/offline/paused/errored) |
| `post_error` | Report an error with severity, triggers alerts |
| `post_tool_call` | Record a tool invocation with name, timing, status, and input/output summaries |
| `list_agents` | List all agents and their current status |
| `get_agent_activity` | Get recent activity timeline for an agent (includes tool calls) |
| `update_prd_embeddings` | Index a PRD document for semantic search |
| `update_design_embeddings` | Index a design document for semantic search |
| `search_embeddings` | Semantic search across indexed documents |
| `list_embedding_sources` | List all indexed embedding sources with metadata |
| `delete_embeddings` | Delete all embeddings for a source |
| `spawn_product_manager` | Spawn a product manager agent with relevant context |
| `spawn_product_designer` | Spawn a product designer agent with relevant context |

## Key Features (Implemented)

- Tool call tracing with purple-coded activity feed entries
- Docker support (Dockerfile + docker-compose.yml with Ollama sidecar)
- Dark mode with system preference detection and localStorage persistence
- Dashboard summary cards with HTMX polling (10s interval)
- Status filter pills for agent table
- Global errors page with pagination
- Alert notification history on alerts page
- Color-coded activity feed with expandable messages
- Agent status indicator dots
- Vector embeddings with sqlite-vec and Ollama
- Semantic search via MCP tools and web UI
- Agent spawning with context retrieval from embeddings
- Multi-channel webhook alerts (Discord, Slack, Generic) with retry logic
- Silent agent detection background task

## Key Constraints

- Keep UI utilitarian — functional ops dashboard, not a marketing site
- All agent interactions must be API key-secured
- Resource usage must be modest (suitable for $4/month VPS)
- No JavaScript frameworks — HTMX + minimal inline JS only
- All styles hand-written in base.html using CSS custom properties
