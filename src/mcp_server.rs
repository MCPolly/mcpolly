use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ServerHandler,
};
use rusqlite::params;
use serde_json::json;
use uuid::Uuid;

use crate::alerts::evaluate_alerts;
use crate::db::DbPool;
use crate::embeddings;
use crate::models::*;

// ─── Parameter structs ───

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RegisterAgentParams {
    #[schemars(description = "Unique name for the agent")]
    name: String,
    #[schemars(description = "Short description of what the agent does")]
    description: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct PostStatusParams {
    #[schemars(description = "Agent ID returned from register_agent")]
    agent_id: String,
    #[schemars(
        description = "One of: starting, running, warning, error, completed, offline, paused, errored"
    )]
    state: String,
    #[schemars(description = "Human-readable message describing what the agent is doing")]
    message: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct PostErrorParams {
    #[schemars(description = "Agent ID returned from register_agent")]
    agent_id: String,
    #[schemars(description = "Error message describing what went wrong")]
    message: String,
    #[schemars(description = "Severity level: error, warning, or critical")]
    severity: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AgentIdParam {
    #[schemars(description = "Agent ID to query")]
    agent_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct UpdateEmbeddingsParams {
    #[schemars(description = "Name identifying this document (e.g. filename or slug)")]
    document_name: String,
    #[schemars(description = "Full markdown content to chunk and embed")]
    content: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchEmbeddingsParams {
    #[schemars(description = "Natural-language search query")]
    query: String,
    #[schemars(description = "Filter by source type (prd, design, etc.)")]
    source_type: Option<String>,
    #[schemars(description = "Number of results to return (default 5)")]
    top_k: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct DeleteEmbeddingsParams {
    #[schemars(description = "Source name to delete all embeddings for")]
    source_name: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SpawnAgentParams {
    #[schemars(description = "Short description of the task for the spawned agent")]
    task: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct StopAgentParams {
    #[schemars(description = "Agent ID to stop")]
    agent_id: String,
    #[schemars(description = "Optional reason for stopping the agent")]
    reason: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct CheckStopParams {
    #[schemars(description = "Agent ID to check for stop signal")]
    agent_id: String,
}

// ─── MCP Handler with direct DB access ───

#[derive(Clone)]
pub struct McPollyHandler {
    db: DbPool,
    tool_router: ToolRouter<Self>,
}

impl McPollyHandler {
    pub fn new(db: DbPool) -> Self {
        Self {
            db,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl McPollyHandler {
    #[tool(
        description = "Register an AI agent with MCPolly. Returns the agent ID needed for subsequent calls. Idempotent on name."
    )]
    async fn register_agent(&self, Parameters(params): Parameters<RegisterAgentParams>) -> String {
        if params.name.trim().is_empty() {
            return json!({"error": "Agent name is required"}).to_string();
        }

        let conn = self.db.get().unwrap();

        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM agents WHERE name = ?1",
                params![params.name.trim()],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing {
            let _ = conn.execute(
                "UPDATE agents SET description = ?1 WHERE id = ?2",
                params![params.description, id],
            );
            json!({"id": id, "name": params.name, "created": false}).to_string()
        } else {
            let id = Uuid::new_v4().to_string();
            match conn.execute(
                "INSERT INTO agents (id, name, description) VALUES (?1, ?2, ?3)",
                params![id, params.name.trim(), params.description],
            ) {
                Ok(_) => json!({"id": id, "name": params.name, "created": true}).to_string(),
                Err(e) => json!({"error": format!("Database error: {e}")}).to_string(),
            }
        }
    }

    #[tool(
        description = "Post a status update for an agent. Valid states: starting, running, warning, error, completed, offline, paused, errored"
    )]
    async fn post_status(&self, Parameters(params): Parameters<PostStatusParams>) -> String {
        if !is_valid_state(&params.state) {
            return json!({"error": format!("Invalid state '{}'. Valid: {:?}", params.state, VALID_STATES)}).to_string();
        }

        let agent_name: Option<String>;
        {
            let conn = self.db.get().unwrap();

            let agent_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM agents WHERE id = ?1",
                    params![params.agent_id],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);

            if !agent_exists {
                return json!({"error": "Agent not found"}).to_string();
            }

            let status_id = Uuid::new_v4().to_string();
            if let Err(e) = conn.execute(
                "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, ?3, ?4)",
                params![status_id, params.agent_id, params.state, params.message],
            ) {
                return json!({"error": format!("Database error: {e}")}).to_string();
            }

            let update_error = if is_error_state(&params.state) {
                format!(
                    ", last_error_message = '{}'",
                    params.message.replace('\'', "''")
                )
            } else {
                String::new()
            };

            let _ = conn.execute(
                &format!(
                    "UPDATE agents SET current_state = ?1, last_message = ?2, last_update_at = datetime('now'){} WHERE id = ?3",
                    update_error
                ),
                params![params.state, params.message, params.agent_id],
            );

            agent_name = conn
                .query_row(
                    "SELECT name FROM agents WHERE id = ?1",
                    params![params.agent_id],
                    |row| row.get(0),
                )
                .ok();
        }

        if is_error_state(&params.state) {
            let name = agent_name.unwrap_or_else(|| params.agent_id.clone());
            evaluate_alerts(
                self.db.clone(),
                "agent_errored",
                Some(&params.agent_id),
                &name,
                &params.message,
            )
            .await;
        }

        // Check for pending stop request
        let stop_requested = {
            let conn = self.db.get().unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM stop_requests WHERE agent_id = ?1 AND status = 'pending'",
                    params![params.agent_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            count > 0
        };

        // If agent posted "stopped", acknowledge the stop request
        if params.state == "stopped" {
            let conn = self.db.get().unwrap();
            conn.execute(
                "UPDATE stop_requests SET status = 'acknowledged', resolved_at = datetime('now') WHERE agent_id = ?1 AND status = 'pending'",
                params![params.agent_id],
            ).ok();
        }

        json!({"status": "ok", "stop_requested": stop_requested}).to_string()
    }

    #[tool(
        description = "Report an error from an agent. Records the error and triggers configured alerts."
    )]
    async fn post_error(&self, Parameters(params): Parameters<PostErrorParams>) -> String {
        if params.message.trim().is_empty() {
            return json!({"error": "Error message is required"}).to_string();
        }

        let agent_name: Option<String>;
        {
            let conn = self.db.get().unwrap();

            let agent_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM agents WHERE id = ?1",
                    params![params.agent_id],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);

            if !agent_exists {
                return json!({"error": "Agent not found"}).to_string();
            }

            let error_id = Uuid::new_v4().to_string();
            if let Err(e) = conn.execute(
                "INSERT INTO errors (id, agent_id, message, severity) VALUES (?1, ?2, ?3, ?4)",
                params![error_id, params.agent_id, params.message, params.severity],
            ) {
                return json!({"error": format!("Database error: {e}")}).to_string();
            }

            let _ = conn.execute(
                "UPDATE agents SET last_error_message = ?1, current_state = 'error', last_update_at = datetime('now') WHERE id = ?2",
                params![params.message, params.agent_id],
            );

            agent_name = conn
                .query_row(
                    "SELECT name FROM agents WHERE id = ?1",
                    params![params.agent_id],
                    |row| row.get(0),
                )
                .ok();
        }

        let name = agent_name.unwrap_or_else(|| params.agent_id.clone());
        evaluate_alerts(
            self.db.clone(),
            "agent_errored",
            Some(&params.agent_id),
            &name,
            &params.message,
        )
        .await;

        json!({"status": "ok"}).to_string()
    }

    #[tool(description = "List all registered agents and their current status")]
    async fn list_agents(&self) -> String {
        let conn = self.db.get().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, name, description, current_state, last_message, last_error_message, registered_at, last_update_at
             FROM agents ORDER BY last_update_at DESC NULLS LAST",
        ) {
            Ok(s) => s,
            Err(e) => return json!({"error": format!("Database error: {e}")}).to_string(),
        };

        let agents: Vec<serde_json::Value> = stmt
            .query_map([], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "name": row.get::<_, String>(1)?,
                    "description": row.get::<_, String>(2)?,
                    "current_state": row.get::<_, String>(3)?,
                    "last_message": row.get::<_, Option<String>>(4)?,
                    "last_error_message": row.get::<_, Option<String>>(5)?,
                    "registered_at": row.get::<_, String>(6)?,
                    "last_update_at": row.get::<_, Option<String>>(7)?,
                }))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        serde_json::to_string(&agents).unwrap_or_else(|_| "[]".to_string())
    }

    #[tool(
        description = "Get the recent activity timeline for a specific agent including status updates and errors"
    )]
    async fn get_agent_activity(&self, Parameters(params): Parameters<AgentIdParam>) -> String {
        let conn = self.db.get().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT * FROM (
                SELECT id, 'status' as event_type, state as severity, message, created_at
                FROM status_updates WHERE agent_id = ?1
                UNION ALL
                SELECT id, 'error' as event_type, severity, message, created_at
                FROM errors WHERE agent_id = ?1
            ) ORDER BY created_at DESC LIMIT 50",
        ) {
            Ok(s) => s,
            Err(e) => return json!({"error": format!("Database error: {e}")}).to_string(),
        };

        let entries: Vec<serde_json::Value> = stmt
            .query_map(params![params.agent_id], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "event_type": row.get::<_, String>(1)?,
                    "severity": row.get::<_, String>(2)?,
                    "message": row.get::<_, String>(3)?,
                    "created_at": row.get::<_, String>(4)?,
                }))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
    }

    #[tool(
        description = "Index a PRD document for semantic search. Chunks the markdown and generates vector embeddings."
    )]
    async fn update_prd_embeddings(
        &self,
        Parameters(params): Parameters<UpdateEmbeddingsParams>,
    ) -> String {
        match embeddings::index_document(&self.db, "prd", &params.document_name, &params.content)
            .await
        {
            Ok(n) => json!({"chunks_indexed": n}).to_string(),
            Err(e) => json!({"error": e}).to_string(),
        }
    }

    #[tool(
        description = "Index a design document for semantic search. Chunks the markdown and generates vector embeddings."
    )]
    async fn update_design_embeddings(
        &self,
        Parameters(params): Parameters<UpdateEmbeddingsParams>,
    ) -> String {
        match embeddings::index_document(&self.db, "design", &params.document_name, &params.content)
            .await
        {
            Ok(n) => json!({"chunks_indexed": n}).to_string(),
            Err(e) => json!({"error": e}).to_string(),
        }
    }

    #[tool(
        description = "Search indexed documents using natural language. Returns the most relevant chunks by semantic similarity."
    )]
    async fn search_embeddings(
        &self,
        Parameters(params): Parameters<SearchEmbeddingsParams>,
    ) -> String {
        let top_k = params.top_k.unwrap_or(5);
        match embeddings::search(
            &self.db,
            &params.query,
            params.source_type.as_deref(),
            top_k,
        )
        .await
        {
            Ok(results) => serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string()),
            Err(e) => json!({"error": e}).to_string(),
        }
    }

    #[tool(
        description = "List all indexed embedding sources with chunk counts and last update times."
    )]
    async fn list_embedding_sources(&self) -> String {
        let conn = self.db.get().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT source_type, source_name, COUNT(*) as chunk_count, MAX(updated_at) as last_updated
             FROM embedding_documents
             GROUP BY source_type, source_name",
        ) {
            Ok(s) => s,
            Err(e) => return json!({"error": format!("Database error: {e}")}).to_string(),
        };

        let sources: Vec<serde_json::Value> = stmt
            .query_map([], |row| {
                Ok(json!({
                    "source_type": row.get::<_, String>(0)?,
                    "source_name": row.get::<_, String>(1)?,
                    "chunk_count": row.get::<_, i64>(2)?,
                    "last_updated": row.get::<_, String>(3)?,
                }))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        serde_json::to_string(&sources).unwrap_or_else(|_| "[]".to_string())
    }

    #[tool(description = "Delete all embeddings for a given source name from both tables.")]
    async fn delete_embeddings(
        &self,
        Parameters(params): Parameters<DeleteEmbeddingsParams>,
    ) -> String {
        let conn = self.db.get().unwrap();

        let doc_ids: Vec<String> = {
            let mut stmt =
                match conn.prepare("SELECT id FROM embedding_documents WHERE source_name = ?1") {
                    Ok(s) => s,
                    Err(e) => return json!({"error": format!("Database error: {e}")}).to_string(),
                };
            stmt.query_map(params![params.source_name], |row| row.get(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        };

        for doc_id in &doc_ids {
            let _ = conn.execute(
                "DELETE FROM vec_embeddings WHERE document_id = ?1",
                params![doc_id],
            );
        }

        let _ = conn.execute(
            "DELETE FROM embedding_documents WHERE source_name = ?1",
            params![params.source_name],
        );

        json!({"deleted": true}).to_string()
    }

    #[tool(
        description = "Spawn a product manager agent. Registers the agent, searches for relevant PRD context, and returns agent_id with context chunks."
    )]
    async fn spawn_product_manager(
        &self,
        Parameters(params): Parameters<SpawnAgentParams>,
    ) -> String {
        let slug = make_slug(&params.task);
        let agent_name = format!("product-manager-{slug}");

        let agent_id = register_or_get_agent(&self.db, &agent_name, &params.task);

        let context: Vec<embeddings::SearchResult> =
            embeddings::search(&self.db, &params.task, None, 5)
                .await
                .unwrap_or_default();

        let conn = self.db.get().unwrap();
        let status_id = Uuid::new_v4().to_string();
        let _ = conn.execute(
            "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, ?3, ?4)",
            params![status_id, agent_id, "starting", params.task],
        );
        let _ = conn.execute(
            "UPDATE agents SET current_state = 'starting', last_message = ?1, last_update_at = datetime('now') WHERE id = ?2",
            params![params.task, agent_id],
        );

        json!({
            "agent_id": agent_id,
            "agent_name": agent_name,
            "context_chunks": context,
        })
        .to_string()
    }

    #[tool(
        description = "Spawn a product designer agent. Registers the agent, searches for relevant PRD and design context, and returns agent_id with context chunks."
    )]
    async fn spawn_product_designer(
        &self,
        Parameters(params): Parameters<SpawnAgentParams>,
    ) -> String {
        let slug = make_slug(&params.task);
        let agent_name = format!("product-designer-{slug}");

        let agent_id = register_or_get_agent(&self.db, &agent_name, &params.task);

        let mut context = Vec::new();
        if let Ok(prd_results) = embeddings::search(&self.db, &params.task, Some("prd"), 3).await {
            context.extend(prd_results);
        }
        if let Ok(design_results) =
            embeddings::search(&self.db, &params.task, Some("design"), 3).await
        {
            context.extend(design_results);
        }

        let conn = self.db.get().unwrap();
        let status_id = Uuid::new_v4().to_string();
        let _ = conn.execute(
            "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, ?3, ?4)",
            params![status_id, agent_id, "starting", params.task],
        );
        let _ = conn.execute(
            "UPDATE agents SET current_state = 'starting', last_message = ?1, last_update_at = datetime('now') WHERE id = ?2",
            params![params.task, agent_id],
        );

        json!({
            "agent_id": agent_id,
            "agent_name": agent_name,
            "context_chunks": context,
        })
        .to_string()
    }

    #[tool(
        description = "Request an agent to stop. Sets the agent to 'stopping' state and creates a stop request that the agent will detect on its next status check."
    )]
    async fn stop_agent(&self, Parameters(params): Parameters<StopAgentParams>) -> String {
        let conn = self.db.get().unwrap();

        let current_state: Option<String> = conn
            .query_row(
                "SELECT current_state FROM agents WHERE id = ?1",
                params![params.agent_id],
                |row| row.get(0),
            )
            .ok();

        let state = match current_state {
            Some(s) => s,
            None => return json!({"error": "Agent not found"}).to_string(),
        };

        if !crate::models::is_stoppable_state(&state) {
            return json!({"error": format!("Agent is in '{}' state and cannot be stopped", state)}).to_string();
        }

        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM stop_requests WHERE agent_id = ?1 AND status = 'pending'",
                params![params.agent_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if pending > 0 {
            return json!({"error": "Agent already has a pending stop request"}).to_string();
        }

        let stop_id = Uuid::new_v4().to_string();
        let reason = params
            .reason
            .unwrap_or_else(|| "Stop requested via MCP".to_string());

        conn.execute(
            "INSERT INTO stop_requests (id, agent_id, requested_by, reason) VALUES (?1, ?2, 'mcp', ?3)",
            params![stop_id, params.agent_id, reason],
        ).ok();

        let status_id = Uuid::new_v4().to_string();
        let msg = format!("Stop requested via MCP: {}", reason);
        conn.execute(
            "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, 'stopping', ?3)",
            params![status_id, params.agent_id, msg],
        ).ok();
        conn.execute(
            "UPDATE agents SET current_state = 'stopping', last_message = ?1, last_update_at = datetime('now') WHERE id = ?2",
            params![msg, params.agent_id],
        ).ok();

        json!({"stop_request_id": stop_id, "status": "pending"}).to_string()
    }

    #[tool(
        description = "Check if a stop signal has been requested for an agent. Returns stop_requested: true if a pending stop exists."
    )]
    async fn check_stop_signal(&self, Parameters(params): Parameters<CheckStopParams>) -> String {
        let conn = self.db.get().unwrap();

        let result = conn.query_row(
            "SELECT reason, created_at FROM stop_requests WHERE agent_id = ?1 AND status = 'pending' LIMIT 1",
            params![params.agent_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );

        match result {
            Ok((reason, requested_at)) => {
                json!({"stop_requested": true, "reason": reason, "requested_at": requested_at})
                    .to_string()
            }
            Err(_) => json!({"stop_requested": false}).to_string(),
        }
    }
}

fn make_slug(task: &str) -> String {
    let slug: String = task
        .split_whitespace()
        .take(4)
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-");
    if slug.len() > 40 {
        slug[..40].to_string()
    } else {
        slug
    }
}

fn register_or_get_agent(db: &DbPool, name: &str, description: &str) -> String {
    let conn = db.get().unwrap();

    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM agents WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        let _ = conn.execute(
            "UPDATE agents SET description = ?1 WHERE id = ?2",
            params![description, id],
        );
        id
    } else {
        let id = Uuid::new_v4().to_string();
        let _ = conn.execute(
            "INSERT INTO agents (id, name, description) VALUES (?1, ?2, ?3)",
            params![id, name, description],
        );
        id
    }
}

#[tool_handler]
impl ServerHandler for McPollyHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "MCPolly: Status and observability for AI agents. \
                 Use register_agent to create an agent, then post_status and post_error to report progress and issues. \
                 Use list_agents and get_agent_activity to inspect state.",
            )
    }
}
