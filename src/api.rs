use axum::{
    extract::{Extension, Path, Query},
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::Deserialize;
use std::sync::OnceLock;
use std::time::Instant;
use uuid::Uuid;

use crate::auth;
use crate::db::DbPool;
use crate::embeddings;
use crate::models::*;

// ─── Server start time tracking ───

static SERVER_START: OnceLock<Instant> = OnceLock::new();

pub fn mark_server_start() {
    SERVER_START.get_or_init(Instant::now);
}

pub fn get_uptime_string() -> String {
    SERVER_START
        .get()
        .map(|start| format_uptime(start.elapsed()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn format_uptime(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if hours >= 24 {
        let days = hours / 24;
        format!("{}d {}h {}m", days, hours % 24, mins)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, mins, s)
    } else {
        format!("{}m {}s", mins, s)
    }
}

fn get_db_size() -> String {
    let db_path = std::env::var("DATABASE_URL").unwrap_or_else(|_| "mcpolly.db".to_string());
    match std::fs::metadata(&db_path) {
        Ok(meta) => {
            let bytes = meta.len();
            if bytes < 1024 {
                format!("{} B", bytes)
            } else if bytes < 1024 * 1024 {
                format!("{:.1} KB", bytes as f64 / 1024.0)
            } else {
                format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
            }
        }
        Err(_) => "unknown".to_string(),
    }
}

async fn check_ollama_connected() -> bool {
    let url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    reqwest::Client::new()
        .get(format!("{}/api/version", url))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// GET /api/v1/server/info — server info endpoint
pub async fn server_info() -> Json<serde_json::Value> {
    let version = std::env::var("MCPOLLY_VERSION").unwrap_or_else(|_| "0.1.1".to_string());

    let uptime = SERVER_START
        .get()
        .map(|start| format_uptime(start.elapsed()))
        .unwrap_or_else(|| "unknown".to_string());

    let db_size = get_db_size();

    let ollama_connected = check_ollama_connected().await;
    let ollama_model =
        std::env::var("OLLAMA_EMBEDDING_MODEL").unwrap_or_else(|_| "all-minilm".to_string());

    Json(serde_json::json!({
        "version": version,
        "uptime": uptime,
        "db_size": db_size,
        "ollama_connected": ollama_connected,
        "ollama_model": ollama_model,
    }))
}

// ─── Agent endpoints (JSON API) ───

#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

/// GET /api/v1/agents — list all agents (JSON)
pub async fn list_agents(
    Extension(db): Extension<DbPool>,
) -> Result<Json<Vec<AgentRow>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, metadata_json, current_state, last_message, last_error_message, registered_at, last_update_at
             FROM agents ORDER BY last_update_at DESC NULLS LAST",
        )
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
        })?;

    let agents: Vec<AgentRow> = stmt
        .query_map([], |row| {
            Ok(AgentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                metadata_json: row.get(3)?,
                current_state: row.get(4)?,
                last_message: row.get(5)?,
                last_error_message: row.get(6)?,
                registered_at: row.get(7)?,
                last_update_at: row.get(8)?,
            })
        })
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(agents))
}

/// GET /api/v1/agents/:id — agent detail (JSON)
pub async fn get_agent(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<Json<AgentRow>, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;
    conn.query_row(
        "SELECT id, name, description, metadata_json, current_state, last_message, last_error_message, registered_at, last_update_at
         FROM agents WHERE id = ?1",
        params![id],
        |row| {
            Ok(AgentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                metadata_json: row.get(3)?,
                current_state: row.get(4)?,
                last_message: row.get(5)?,
                last_error_message: row.get(6)?,
                registered_at: row.get(7)?,
                last_update_at: row.get(8)?,
            })
        },
    )
    .map(Json)
    .map_err(|_| {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Agent not found"})))
    })
}

/// GET /api/v1/agents/:id/activity — activity timeline (JSON)
pub async fn get_agent_activity(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(50).min(100);
    let offset = params.offset.unwrap_or(0);
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let mut stmt = conn
        .prepare(
            "SELECT * FROM (
                SELECT id, 'status' as event_type, state as severity, message, created_at
                FROM status_updates WHERE agent_id = ?1
                UNION ALL
                SELECT id, 'error' as event_type, severity, message, created_at
                FROM errors WHERE agent_id = ?1
                UNION ALL
                SELECT id, 'tool_call' as event_type, status as severity,
                    tool_name || COALESCE(' — ' || input_summary, '') as message,
                    created_at
                FROM tool_calls WHERE agent_id = ?1
            ) ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
        )
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?;

    let entries: Vec<serde_json::Value> = stmt
        .query_map(params![id, limit, offset], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "event_type": row.get::<_, String>(1)?,
                "severity": row.get::<_, String>(2)?,
                "message": row.get::<_, String>(3)?,
                "created_at": row.get::<_, String>(4)?,
            }))
        })
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(entries))
}

/// GET /api/v1/agents/:id/tool-calls — tool calls for a specific agent (JSON)
pub async fn get_agent_tool_calls(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<ToolCall>>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(50).min(100);
    let offset = params.offset.unwrap_or(0);
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let mut stmt = conn
        .prepare(
            "SELECT id, agent_id, tool_name, input_summary, output_summary, duration_ms, status, parent_span_id, created_at
             FROM tool_calls WHERE agent_id = ?1
             ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
        )
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?;

    let calls: Vec<ToolCall> = stmt
        .query_map(params![id, limit, offset], |row| {
            Ok(ToolCall {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                tool_name: row.get(2)?,
                input_summary: row.get(3)?,
                output_summary: row.get(4)?,
                duration_ms: row.get(5)?,
                status: row.get(6)?,
                parent_span_id: row.get(7)?,
                created_at: row.get(8)?,
            })
        })
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(calls))
}

/// GET /api/v1/agents/:id/errors — errors for a specific agent (JSON)
pub async fn get_agent_errors(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<AgentError>>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(50).min(100);
    let offset = params.offset.unwrap_or(0);
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let mut stmt = conn
        .prepare(
            "SELECT id, agent_id, message, severity, stack_trace, created_at
             FROM errors WHERE agent_id = ?1
             ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
        )
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?;

    let errors: Vec<AgentError> = stmt
        .query_map(params![id, limit, offset], |row| {
            Ok(AgentError {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                message: row.get(2)?,
                severity: row.get(3)?,
                stack_trace: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(errors))
}

#[derive(Debug, Deserialize)]
pub struct SetStatusRequest {
    pub state: String,
    #[serde(default)]
    pub message: String,
}

/// PUT /api/v1/agents/:id/status — manually set agent status
pub async fn set_agent_status(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
    Json(req): Json<SetStatusRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_state(&req.state) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": format!("Invalid state '{}'. Valid states: {:?}", req.state, VALID_STATES)}),
            ),
        ));
    }

    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let agent_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM agents WHERE id = ?1",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !agent_exists {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Agent not found"})),
        ));
    }

    let status_message = if req.message.trim().is_empty() {
        format!("Status manually set to {}", req.state)
    } else {
        format!(
            "Status manually set to {}: {}",
            req.state,
            req.message.trim()
        )
    };

    let status_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, ?3, ?4)",
        params![status_id, id, req.state, status_message],
    )
    .map_err(|e| {
        tracing::error!("Insert error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database error"})),
        )
    })?;

    let update_error = if is_error_state(&req.state) {
        format!(
            ", last_error_message = '{}'",
            status_message.replace('\'', "''")
        )
    } else {
        String::new()
    };

    conn.execute(
        &format!(
            "UPDATE agents SET current_state = ?1, last_message = ?2, last_update_at = datetime('now'){} WHERE id = ?3",
            update_error
        ),
        params![req.state, status_message, id],
    )
    .map_err(|e| {
        tracing::error!("Update error: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
    })?;

    // Check for pending stop request
    let stop_requested: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM stop_requests WHERE agent_id = ?1 AND status = 'pending'",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    // If agent posted "stopped", acknowledge the stop request
    if req.state == "stopped" {
        conn.execute(
            "UPDATE stop_requests SET status = 'acknowledged', resolved_at = datetime('now') WHERE agent_id = ?1 AND status = 'pending'",
            params![id],
        ).ok();
    }

    Ok(Json(
        serde_json::json!({"status": "ok", "state": req.state, "stop_requested": stop_requested}),
    ))
}

// ─── Agent stop endpoints ───

#[derive(Debug, Deserialize)]
pub struct StopAgentRequest {
    #[serde(default)]
    pub reason: String,
    #[serde(default = "default_requested_by")]
    pub requested_by: String,
}

fn default_requested_by() -> String {
    "operator".to_string()
}

/// POST /api/v1/agents/:id/stop — request agent stop
pub async fn stop_agent(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
    Json(req): Json<StopAgentRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let current_state: String = conn
        .query_row(
            "SELECT current_state FROM agents WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(|_| {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Agent not found"})),
            )
        })?;

    if !is_stoppable_state(&current_state) {
        return Err((
            StatusCode::CONFLICT,
            Json(
                serde_json::json!({"error": format!("Agent is in '{}' state and cannot be stopped", current_state)}),
            ),
        ));
    }

    let pending_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM stop_requests WHERE agent_id = ?1 AND status = 'pending'",
            params![id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if pending_count > 0 {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "Agent already has a pending stop request"})),
        ));
    }

    let stop_id = Uuid::new_v4().to_string();
    let reason = if req.reason.trim().is_empty() {
        "Stop requested".to_string()
    } else {
        req.reason.trim().to_string()
    };

    conn.execute(
        "INSERT INTO stop_requests (id, agent_id, requested_by, reason) VALUES (?1, ?2, ?3, ?4)",
        params![stop_id, id, req.requested_by, reason],
    )
    .map_err(|e| {
        tracing::error!("Insert error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database error"})),
        )
    })?;

    let status_id = Uuid::new_v4().to_string();
    let msg = format!("Stop requested by {}: {}", req.requested_by, reason);
    conn.execute(
        "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, 'stopping', ?3)",
        params![status_id, id, msg],
    )
    .ok();
    conn.execute(
        "UPDATE agents SET current_state = 'stopping', last_message = ?1, last_update_at = datetime('now') WHERE id = ?2",
        params![msg, id],
    ).ok();

    Ok(Json(
        serde_json::json!({"stop_request_id": stop_id, "status": "pending"}),
    ))
}

/// DELETE /api/v1/agents/:id/stop — cancel pending stop request
pub async fn cancel_stop_agent(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let affected = conn.execute(
        "UPDATE stop_requests SET status = 'cancelled', resolved_at = datetime('now') WHERE agent_id = ?1 AND status = 'pending'",
        params![id],
    ).map_err(|e| {
        tracing::error!("Update error: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
    })?;

    if affected == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "No pending stop request found"})),
        ));
    }

    // Revert agent to previous non-stopping state
    let prev_state: String = conn
        .query_row(
            "SELECT state FROM status_updates WHERE agent_id = ?1 AND state != 'stopping' ORDER BY created_at DESC LIMIT 1",
            params![id], |row| row.get(0),
        )
        .unwrap_or_else(|_| "running".to_string());

    let status_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, ?3, 'Stop request cancelled')",
        params![status_id, id, prev_state],
    ).ok();
    conn.execute(
        "UPDATE agents SET current_state = ?1, last_message = 'Stop request cancelled', last_update_at = datetime('now') WHERE id = ?2",
        params![prev_state, id],
    ).ok();

    Ok(Json(
        serde_json::json!({"status": "cancelled", "reverted_to": prev_state}),
    ))
}

/// GET /api/v1/agents/:id/stop — get current stop request status
pub async fn get_stop_status(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let result = conn.query_row(
        "SELECT id, requested_by, reason, status, created_at, resolved_at FROM stop_requests WHERE agent_id = ?1 ORDER BY created_at DESC LIMIT 1",
        params![id],
        |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "requested_by": row.get::<_, String>(1)?,
                "reason": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?,
                "created_at": row.get::<_, String>(4)?,
                "resolved_at": row.get::<_, Option<String>>(5)?,
            }))
        },
    );

    match result {
        Ok(val) => Ok(Json(val)),
        Err(_) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "No stop request found"})),
        )),
    }
}

// ─── Alert JSON endpoints ───

/// GET /api/v1/alerts — list alert rules (JSON)
pub async fn list_alerts(
    Extension(db): Extension<DbPool>,
) -> Result<Json<Vec<AlertRule>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, condition, agent_id, webhook_url, channel_type, enabled, silence_minutes, created_at
             FROM alert_rules ORDER BY created_at DESC",
        )
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
        })?;

    let rules: Vec<AlertRule> = stmt
        .query_map([], |row| {
            Ok(AlertRule {
                id: row.get(0)?,
                name: row.get(1)?,
                condition: row.get(2)?,
                agent_id: row.get(3)?,
                webhook_url: row.get(4)?,
                channel_type: row
                    .get::<_, String>(5)
                    .unwrap_or_else(|_| "discord".to_string()),
                enabled: row.get::<_, i64>(6)? != 0,
                silence_minutes: row.get(7)?,
                created_at: row.get(8)?,
            })
        })
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(rules))
}

/// POST /api/v1/alerts — create alert rule (JSON)
pub async fn create_alert(
    Extension(db): Extension<DbPool>,
    Json(req): Json<CreateAlertRuleRequest>,
) -> Result<(StatusCode, Json<AlertRule>), (StatusCode, Json<serde_json::Value>)> {
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Rule name is required"})),
        ));
    }

    const VALID_CONDITIONS: &[&str] = &[
        "agent_error",
        "agent_errored",
        "agent_offline",
        "agent_silent",
        "agent_completed",
        "agent_running",
        "agent_starting",
        "agent_warning",
        "agent_paused",
        "agent_stopped",
        "agent_stopping",
        "any_status",
    ];
    if !VALID_CONDITIONS.contains(&req.condition.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": format!("Invalid condition '{}'. Valid: {:?}", req.condition, VALID_CONDITIONS)}),
            ),
        ));
    }

    if req.webhook_url.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Webhook URL is required"})),
        ));
    }

    let id = Uuid::new_v4().to_string();
    let silence_minutes = req.silence_minutes.unwrap_or(5);
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let channel_type = if ["discord", "slack", "generic"].contains(&req.channel_type.as_str()) {
        req.channel_type.clone()
    } else {
        "discord".to_string()
    };

    conn.execute(
        "INSERT INTO alert_rules (id, name, condition, agent_id, webhook_url, channel_type, enabled, silence_minutes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            req.name.trim(),
            req.condition,
            req.agent_id,
            req.webhook_url.trim(),
            channel_type,
            req.enabled as i64,
            silence_minutes,
        ],
    )
    .map_err(|e| {
        tracing::error!("Failed to create alert rule: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
    })?;

    let rule = AlertRule {
        id,
        name: req.name,
        condition: req.condition,
        agent_id: req.agent_id,
        webhook_url: req.webhook_url,
        channel_type,
        enabled: req.enabled,
        silence_minutes,
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };

    Ok((StatusCode::CREATED, Json(rule)))
}

/// DELETE /api/v1/alerts/:id — delete alert rule (JSON)
pub async fn delete_alert(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;
    conn.execute(
        "DELETE FROM alert_history WHERE alert_rule_id = ?1",
        params![id],
    )
    .map_err(|e| {
        tracing::error!("Failed to delete alert history: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database error"})),
        )
    })?;
    let affected = conn
        .execute("DELETE FROM alert_rules WHERE id = ?1", params![id])
        .map_err(|e| {
            tracing::error!("Failed to delete alert rule: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?;

    if affected == 0 {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Alert rule not found"})),
        ))
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

/// GET /api/v1/alerts/history — alert history (JSON)
pub async fn list_alert_history(
    Extension(db): Extension<DbPool>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<AlertHistoryEntry>>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.limit.unwrap_or(50).min(100);
    let offset = params.offset.unwrap_or(0);
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let mut stmt = conn
        .prepare(
            "SELECT id, alert_rule_id, agent_id, agent_name, condition, message, delivery_status, created_at
             FROM alert_history ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        )
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
        })?;

    let entries: Vec<AlertHistoryEntry> = stmt
        .query_map(params![limit, offset], |row| {
            Ok(AlertHistoryEntry {
                id: row.get(0)?,
                alert_rule_id: row.get(1)?,
                agent_id: row.get(2)?,
                agent_name: row.get(3)?,
                condition: row.get(4)?,
                message: row.get(5)?,
                delivery_status: row.get(6)?,
                created_at: row.get(7)?,
            })
        })
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(entries))
}

// ─── API Key management endpoints (JSON) ───

/// GET /api/v1/keys — list API keys
pub async fn list_api_keys(
    Extension(db): Extension<DbPool>,
) -> Result<Json<Vec<ApiKeyRow>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;
    let mut stmt = conn
        .prepare(
            "SELECT id, label, key_prefix, created_at, last_used_at, revoked
             FROM api_keys ORDER BY created_at DESC",
        )
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?;

    let keys: Vec<ApiKeyRow> = stmt
        .query_map([], |row| {
            Ok(ApiKeyRow {
                id: row.get(0)?,
                label: row.get(1)?,
                key_prefix: row.get(2)?,
                created_at: row.get(3)?,
                last_used_at: row.get(4)?,
                revoked: row.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(keys))
}

/// POST /api/v1/keys — generate new API key (JSON)
pub async fn create_api_key(
    Extension(db): Extension<DbPool>,
    Json(req): Json<CreateApiKeyRequest>,
) -> Result<(StatusCode, Json<CreateApiKeyResponse>), (StatusCode, Json<serde_json::Value>)> {
    let raw_key = auth::generate_raw_key();
    let key_hash = auth::hash_key(&raw_key);
    let key_prefix = raw_key[raw_key.len().saturating_sub(4)..].to_string();
    let id = Uuid::new_v4().to_string();
    let label = if req.label.trim().is_empty() {
        "unnamed".to_string()
    } else {
        req.label.trim().to_string()
    };

    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;
    conn.execute(
        "INSERT INTO api_keys (id, label, key_hash, key_prefix) VALUES (?1, ?2, ?3, ?4)",
        params![id, label, key_hash, key_prefix],
    )
    .map_err(|e| {
        tracing::error!("Failed to create API key: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database error"})),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            id,
            label,
            key: raw_key,
            key_prefix,
        }),
    ))
}

/// DELETE /api/v1/keys/:id — revoke API key (JSON)
pub async fn revoke_api_key(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let active_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM api_keys WHERE revoked = 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if active_count <= 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Cannot revoke the last active API key"})),
        ));
    }

    let affected = conn
        .execute(
            "UPDATE api_keys SET revoked = 1 WHERE id = ?1 AND revoked = 0",
            params![id],
        )
        .map_err(|e| {
            tracing::error!("Failed to revoke API key: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?;

    if affected == 0 {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "API key not found or already revoked"})),
        ))
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

// ─── Embeddings JSON endpoints ───

#[derive(Debug, Deserialize)]
pub struct IndexEmbeddingsRequest {
    pub source_type: String,
    pub source_name: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchEmbeddingsQuery {
    pub q: String,
    pub source_type: Option<String>,
    pub top_k: Option<i64>,
}

/// POST /api/v1/embeddings/index — chunk and index a document
pub async fn index_embeddings(
    Extension(db): Extension<DbPool>,
    Json(req): Json<IndexEmbeddingsRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if req.source_type.trim().is_empty() || req.source_name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "source_type and source_name are required"})),
        ));
    }

    match embeddings::index_document(&db, &req.source_type, &req.source_name, &req.content).await {
        Ok(count) => Ok(Json(serde_json::json!({"chunks_indexed": count}))),
        Err(e) => {
            tracing::error!("Embedding index error: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e})),
            ))
        }
    }
}

/// GET /api/v1/embeddings/search — semantic search across indexed documents
pub async fn search_embeddings_api(
    Extension(db): Extension<DbPool>,
    Query(params): Query<SearchEmbeddingsQuery>,
) -> Result<Json<Vec<embeddings::SearchResult>>, (StatusCode, Json<serde_json::Value>)> {
    let top_k = params.top_k.unwrap_or(5);

    match embeddings::search(&db, &params.q, params.source_type.as_deref(), top_k).await {
        Ok(results) => Ok(Json(results)),
        Err(e) => {
            tracing::error!("Embedding search error: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e})),
            ))
        }
    }
}

/// GET /api/v1/embeddings/sources — list all indexed sources with chunk counts
pub async fn list_embedding_sources(
    Extension(db): Extension<DbPool>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let mut stmt = conn
        .prepare(
            "SELECT source_type, source_name, COUNT(*) as chunk_count, MAX(updated_at) as last_updated
             FROM embedding_documents
             GROUP BY source_type, source_name",
        )
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?;

    let sources: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "source_type": row.get::<_, String>(0)?,
                "source_name": row.get::<_, String>(1)?,
                "chunk_count": row.get::<_, i64>(2)?,
                "last_updated": row.get::<_, String>(3)?,
            }))
        })
        .map_err(|e| {
            tracing::error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(sources))
}

/// DELETE /api/v1/embeddings/sources/:name — delete all embeddings for a source
pub async fn delete_embedding_source(
    Extension(db): Extension<DbPool>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;

    let doc_ids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM embedding_documents WHERE source_name = ?1")
            .map_err(|e| {
                tracing::error!("Query error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "Database error"})),
                )
            })?;
        let ids = stmt
            .query_map(params![name], |row| row.get(0))
            .map_err(|e| {
                tracing::error!("Query error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "Database error"})),
                )
            })?
            .filter_map(|r| r.ok())
            .collect();
        ids
    };

    if doc_ids.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "No embeddings found for this source"})),
        ));
    }

    for doc_id in &doc_ids {
        let _ = conn.execute(
            "DELETE FROM vec_embeddings WHERE document_id = ?1",
            params![doc_id],
        );
    }

    conn.execute(
        "DELETE FROM embedding_documents WHERE source_name = ?1",
        params![name],
    )
    .map_err(|e| {
        tracing::error!("Delete error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database error"})),
        )
    })?;

    Ok(StatusCode::NO_CONTENT)
}
