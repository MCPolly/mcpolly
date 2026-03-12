use axum::{extract::Extension, http::StatusCode, Json};
use rusqlite::params;
use uuid::Uuid;

use crate::alerts::evaluate_alerts;
use crate::db::DbPool;
use crate::models::*;

/// POST /api/v1/agents/register
/// Register or update an agent. Idempotent on name.
pub async fn register_agent(
    Extension(db): Extension<DbPool>,
    Json(req): Json<RegisterAgentRequest>,
) -> Result<(StatusCode, Json<RegisterAgentResponse>), (StatusCode, Json<serde_json::Value>)> {
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Agent name is required"})),
        ));
    }

    let conn = db.get().map_err(|e| {
        tracing::error!("Pool error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Database pool error"})),
        )
    })?;
    let metadata_json = req
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());

    // Check if agent already exists by name
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM agents WHERE name = ?1",
            params![req.name.trim()],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        // Update description and metadata
        conn.execute(
            "UPDATE agents SET description = ?1, metadata_json = ?2 WHERE id = ?3",
            params![req.description, metadata_json, id],
        )
        .map_err(|e| {
            tracing::error!("Failed to update agent: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?;

        Ok((
            StatusCode::OK,
            Json(RegisterAgentResponse {
                id,
                name: req.name,
                created: false,
            }),
        ))
    } else {
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agents (id, name, description, metadata_json) VALUES (?1, ?2, ?3, ?4)",
            params![id, req.name.trim(), req.description, metadata_json],
        )
        .map_err(|e| {
            tracing::error!("Failed to insert agent: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database error"})),
            )
        })?;

        Ok((
            StatusCode::CREATED,
            Json(RegisterAgentResponse {
                id,
                name: req.name,
                created: true,
            }),
        ))
    }
}

/// POST /api/v1/status
/// Post a status update for an agent.
pub async fn post_status(
    Extension(db): Extension<DbPool>,
    Json(req): Json<PostStatusRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_state(&req.state) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("Invalid state '{}'. Valid states: {:?}", req.state, VALID_STATES)
            })),
        ));
    }

    let message = req.message.clone().unwrap_or_default();
    let metadata_json = req
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());

    let agent_name: Option<String>;
    {
        let conn = db.get().map_err(|e| {
            tracing::error!("Pool error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database pool error"})),
            )
        })?;

        // Verify agent exists
        let agent_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE id = ?1",
                params![req.agent_id],
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

        let status_id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO status_updates (id, agent_id, state, message, metadata_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![status_id, req.agent_id, req.state, message, metadata_json],
        )
        .map_err(|e| {
            tracing::error!("Failed to insert status update: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
        })?;

        // Update agent's current state and last message
        let update_error_msg = if is_error_state(&req.state) {
            format!(", last_error_message = '{}'", message.replace('\'', "''"))
        } else {
            String::new()
        };

        conn.execute(
            &format!(
                "UPDATE agents SET current_state = ?1, last_message = ?2, last_update_at = datetime('now'){} WHERE id = ?3",
                update_error_msg
            ),
            params![req.state, message, req.agent_id],
        )
        .map_err(|e| {
            tracing::error!("Failed to update agent state: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
        })?;

        agent_name = conn
            .query_row(
                "SELECT name FROM agents WHERE id = ?1",
                params![req.agent_id],
                |row| row.get(0),
            )
            .ok();
    }

    // Evaluate alert rules for this status change
    {
        let name = agent_name.unwrap_or_else(|| req.agent_id.clone());
        let condition = format!("agent_{}", req.state);
        evaluate_alerts(
            db.clone(),
            &condition,
            Some(&req.agent_id),
            &name,
            &message,
        )
        .await;
    }

    Ok((StatusCode::OK, Json(serde_json::json!({"status": "ok"}))))
}

/// POST /api/v1/errors
/// Report an error from an agent.
pub async fn post_error(
    Extension(db): Extension<DbPool>,
    Json(req): Json<PostErrorRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    if req.message.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Error message is required"})),
        ));
    }

    let agent_name: Option<String>;
    {
        let conn = db.get().map_err(|e| {
            tracing::error!("Pool error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database pool error"})),
            )
        })?;

        // Verify agent exists
        let agent_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE id = ?1",
                params![req.agent_id],
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

        let error_id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO errors (id, agent_id, message, severity, stack_trace) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![error_id, req.agent_id, req.message, req.severity, req.stack_trace],
        )
        .map_err(|e| {
            tracing::error!("Failed to insert error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
        })?;

        // Update agent's last error message and state
        conn.execute(
            "UPDATE agents SET last_error_message = ?1, current_state = 'error', last_update_at = datetime('now') WHERE id = ?2",
            params![req.message, req.agent_id],
        )
        .map_err(|e| {
            tracing::error!("Failed to update agent error state: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Database error"})))
        })?;

        agent_name = conn
            .query_row(
                "SELECT name FROM agents WHERE id = ?1",
                params![req.agent_id],
                |row| row.get(0),
            )
            .ok();
    }

    // Evaluate alert rules
    let name = agent_name.unwrap_or_else(|| req.agent_id.clone());
    evaluate_alerts(
        db.clone(),
        "agent_errored",
        Some(&req.agent_id),
        &name,
        &req.message,
    )
    .await;

    Ok((StatusCode::OK, Json(serde_json::json!({"status": "ok"}))))
}
