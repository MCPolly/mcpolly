use askama::Template;
use axum::{
    extract::Extension,
    extract::{Path, Query},
    http::{self, StatusCode},
    response::{Html, IntoResponse, Response},
};
use rusqlite::params;
use serde::Deserialize;

use crate::db::DbPool;
use crate::models::*;

// ─── Pagination params ───

#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

// ─── Template structs ───
// Field names must match what the askama templates reference.

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub active_nav: String,
    pub agents: Vec<AgentView>,
    pub total_count: usize,
    pub running_count: usize,
    pub errored_count: usize,
    pub offline_count: usize,
}

#[derive(Template)]
#[template(path = "agent_detail.html")]
pub struct AgentDetailTemplate {
    pub active_nav: String,
    pub agent: AgentView,
    pub activities: Vec<ActivityEntry>,
    pub has_more: bool,
    pub next_offset: i64,
}

#[derive(Template)]
#[template(path = "activity_fragment.html")]
pub struct ActivityFragmentTemplate {
    pub entries: Vec<ActivityEntry>,
    pub agent_id: String,
    pub next_offset: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AlertHistoryView {
    pub agent_id: String,
    pub agent_name: String,
    pub rule_condition: String,
    pub message: String,
    pub status: String,
    pub sent_at: String,
}

#[derive(Template)]
#[template(path = "alerts.html")]
pub struct AlertsTemplate {
    pub active_nav: String,
    pub alerts: Vec<AlertView>,
    #[allow(dead_code)]
    pub agents: Vec<AgentView>,
    pub alert_history: Vec<AlertHistoryView>,
}

#[derive(Template)]
#[template(path = "alerts_form.html")]
pub struct AlertsFormTemplate {
    pub agents: Vec<AgentView>,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTemplate {
    pub active_nav: String,
    pub keys: Vec<ApiKeyView>,
    pub new_key: Option<String>,
}

#[derive(Template)]
#[template(path = "settings_key_form.html")]
pub struct SettingsKeyFormTemplate {}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    pub error: Option<String>,
}

// ─── Embeddings ───

#[derive(Debug, Clone)]
pub struct EmbeddingSourceView {
    pub source_type: String,
    pub source_name: String,
    pub chunk_count: i64,
    pub last_updated: String,
}

#[derive(Debug, Clone)]
pub struct SearchResultView {
    pub heading: String,
    pub content: String,
    pub source_name: String,
    pub source_type: String,
    pub distance: String,
}

#[derive(Template)]
#[template(path = "embeddings.html")]
pub struct EmbeddingsTemplate {
    pub active_nav: String,
    pub sources: Vec<EmbeddingSourceView>,
}

#[derive(Template)]
#[template(path = "embeddings_search_results.html")]
pub struct EmbeddingsSearchResultsTemplate {
    pub results: Vec<SearchResultView>,
    pub query: String,
}

// ─── Errors page ───

#[derive(Debug, Clone)]
pub struct ErrorView {
    pub agent_id: String,
    pub agent_name: String,
    pub severity: String,
    pub message: String,
    pub timestamp: String,
}

#[derive(Template)]
#[template(path = "errors.html")]
pub struct ErrorsTemplate {
    pub active_nav: String,
    pub errors: Vec<ErrorView>,
    pub total_errors: i64,
    pub has_more: bool,
    pub next_offset: i64,
}

// ─── Status filter params ───

#[derive(Debug, Deserialize)]
pub struct StatusFilterParams {
    pub status: Option<String>,
}

// ─── DB query helpers ───

fn query_agent_rows(conn: &rusqlite::Connection) -> Vec<AgentRow> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, metadata_json, current_state, last_message, last_error_message, registered_at, last_update_at
             FROM agents ORDER BY
                CASE current_state
                    WHEN 'error' THEN 0 WHEN 'errored' THEN 0
                    WHEN 'warning' THEN 1
                    WHEN 'offline' THEN 2
                    ELSE 3
                END, last_update_at DESC NULLS LAST",
        )
        .unwrap();

    stmt.query_map([], |row| {
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
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

fn agent_row_to_view(conn: &rusqlite::Connection, row: &AgentRow) -> AgentView {
    let total_updates: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM status_updates WHERE agent_id = ?1",
            params![row.id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let total_errors: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM errors WHERE agent_id = ?1",
            params![row.id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    row.to_view(total_updates, total_errors)
}

fn query_activity(
    conn: &rusqlite::Connection,
    agent_id: &str,
    limit: i64,
    offset: i64,
) -> Vec<ActivityEntry> {
    let mut stmt = conn
        .prepare(
            "SELECT * FROM (
                SELECT 'status' as entry_type, state, message, created_at
                FROM status_updates WHERE agent_id = ?1
                UNION ALL
                SELECT 'error' as entry_type, severity as state, message, created_at
                FROM errors WHERE agent_id = ?1
            ) ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
        )
        .unwrap();

    stmt.query_map(params![agent_id, limit, offset], |row| {
        let entry_type: String = row.get(0)?;
        let state_val: String = row.get(1)?;
        let state = if entry_type == "status" {
            Some(state_val)
        } else {
            None
        };
        Ok(ActivityEntry {
            timestamp: row.get::<_, String>(3)?,
            entry_type,
            state,
            message: row.get(2)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

// ─── Route handlers ───

/// GET / — dashboard
pub async fn dashboard(Extension(db): Extension<DbPool>) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = query_agent_rows(&conn);
    let agents: Vec<AgentView> = rows.iter().map(|r| agent_row_to_view(&conn, r)).collect();

    let total_count = agents.len();
    let running_count = agents.iter().filter(|a| a.status == "running").count();
    let errored_count = agents
        .iter()
        .filter(|a| a.status == "errored" || a.status == "error")
        .count();
    let offline_count = agents
        .iter()
        .filter(|a| a.status == "offline" || a.status == "completed")
        .count();

    let template = DashboardTemplate {
        active_nav: "agents".to_string(),
        agents,
        total_count,
        running_count,
        errored_count,
        offline_count,
    };
    Ok(template.into_response())
}

/// GET /partials/agents — HTMX partial for agent table rows (polling)
pub async fn agents_partial(
    Extension(db): Extension<DbPool>,
    Query(params): Query<StatusFilterParams>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = query_agent_rows(&conn);
    let agents: Vec<AgentView> = rows
        .iter()
        .map(|r| agent_row_to_view(&conn, r))
        .filter(|a| {
            if let Some(ref status) = params.status {
                if status.is_empty() {
                    return true;
                }
                a.status == *status
            } else {
                true
            }
        })
        .collect();

    // Return just the table row HTML fragments
    let mut html = String::new();
    for agent in &agents {
        html.push_str(&format!(
            r#"<tr>
  <td><a href="/agents/{id}">{name}</a></td>
  <td><span class="badge badge-{status}">{status}</span></td>
  <td class="mono">{last_seen}</td>
  <td>{last_error}</td>
  <td><a href="/agents/{id}" class="btn btn-sm">View</a></td>
</tr>"#,
            id = agent.id,
            name = agent.name,
            status = agent.status,
            last_seen = agent.last_seen,
            last_error = agent.last_error.as_deref().map(|e| {
                let truncated = if e.len() > 60 { &e[..60] } else { e };
                format!(r#"<span class="truncate" style="display: inline-block;" title="{}">{}</span>"#, e, truncated)
            }).unwrap_or_else(|| r#"<span class="text-muted">&mdash;</span>"#.to_string()),
        ));
    }

    Ok(Html(html).into_response())
}

/// GET /partials/summary — HTMX partial for summary cards
pub async fn summary_partial(Extension(db): Extension<DbPool>) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = query_agent_rows(&conn);

    let total = rows.len();
    let running = rows.iter().filter(|r| r.current_state == "running").count();
    let errored = rows
        .iter()
        .filter(|r| r.current_state == "errored" || r.current_state == "error")
        .count();
    let offline = rows
        .iter()
        .filter(|r| r.current_state == "offline" || r.current_state == "completed")
        .count();

    let html = format!(
        r#"<div class="summary-card">
  <div class="summary-card-value">{}</div>
  <div class="summary-card-label">Total Agents</div>
</div>
<div class="summary-card summary-card-running">
  <div class="summary-card-value">{}</div>
  <div class="summary-card-label">Running</div>
</div>
<div class="summary-card summary-card-errored">
  <div class="summary-card-value">{}</div>
  <div class="summary-card-label">Errored</div>
</div>
<div class="summary-card summary-card-offline">
  <div class="summary-card-value">{}</div>
  <div class="summary-card-label">Offline</div>
</div>"#,
        total, running, errored, offline
    );

    Ok(Html(html).into_response())
}

/// GET /agents/:id — agent detail page
pub async fn agent_detail(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let row = conn
        .query_row(
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
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let agent = agent_row_to_view(&conn, &row);

    let limit: i64 = 50;
    let all_activity = query_activity(&conn, &id, limit + 1, 0);
    let has_more = all_activity.len() as i64 > limit;
    let activities: Vec<ActivityEntry> = all_activity.into_iter().take(limit as usize).collect();

    let template = AgentDetailTemplate {
        active_nav: "agents".to_string(),
        agent,
        activities,
        has_more,
        next_offset: limit,
    };
    Ok(template.into_response())
}

/// GET /agents/:id/activity — HTMX partial for loading more activity
pub async fn agent_activity_fragment(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Result<Response, StatusCode> {
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(50);

    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let all_activity = query_activity(&conn, &id, limit + 1, offset);
    let has_more = all_activity.len() as i64 > limit;
    let entries: Vec<ActivityEntry> = all_activity.into_iter().take(limit as usize).collect();

    let template = ActivityFragmentTemplate {
        entries,
        agent_id: id,
        next_offset: offset + limit,
        has_more,
    };
    Ok(template.into_response())
}

/// GET /alerts — alerts page
pub async fn alerts_page(Extension(db): Extension<DbPool>) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut rules_stmt = conn
        .prepare(
            "SELECT ar.id, ar.condition, ar.agent_id, ar.webhook_url, ar.enabled, ar.created_at, a.name
             FROM alert_rules ar LEFT JOIN agents a ON ar.agent_id = a.id
             ORDER BY ar.created_at DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let alerts: Vec<AlertView> = rules_stmt
        .query_map([], |row| {
            Ok(AlertView {
                id: row.get(0)?,
                condition: row.get(1)?,
                agent_name: row.get(6)?,
                webhook_url: row.get(3)?,
                active: row.get::<_, i64>(4)? != 0,
                created_at: row.get(5)?,
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter_map(|r| r.ok())
        .collect();

    let rows = query_agent_rows(&conn);
    let agents: Vec<AgentView> = rows.iter().map(|r| agent_row_to_view(&conn, r)).collect();

    let mut hist_stmt = conn
        .prepare(
            "SELECT ah.agent_id, COALESCE(ah.agent_name, 'unknown'), ah.condition, ah.message, ah.delivery_status, ah.created_at
             FROM alert_history ah ORDER BY ah.created_at DESC LIMIT 20",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let alert_history: Vec<AlertHistoryView> = hist_stmt
        .query_map([], |row| {
            Ok(AlertHistoryView {
                agent_id: row.get::<_, String>(0).unwrap_or_default(),
                agent_name: row.get(1)?,
                rule_condition: row.get(2)?,
                message: row.get(3)?,
                status: row.get(4)?,
                sent_at: row
                    .get::<_, String>(5)
                    .map(|ts| crate::models::relative_time(&ts))
                    .unwrap_or_else(|_| "unknown".to_string()),
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter_map(|r| r.ok())
        .collect();

    let template = AlertsTemplate {
        active_nav: "alerts".to_string(),
        alerts,
        agents,
        alert_history,
    };
    Ok(template.into_response())
}

/// GET /alerts/new-form — HTMX partial for new alert form
pub async fn alerts_new_form(Extension(db): Extension<DbPool>) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = query_agent_rows(&conn);
    let agents: Vec<AgentView> = rows.iter().map(|r| agent_row_to_view(&conn, r)).collect();

    let template = AlertsFormTemplate { agents };
    Ok(template.into_response())
}

/// GET /alerts/cancel-form — returns empty to clear form area
pub async fn alerts_cancel_form() -> Html<String> {
    Html(String::new())
}

/// POST /alerts — create alert from form
pub async fn create_alert_form(
    Extension(db): Extension<DbPool>,
    axum::extract::Form(form): axum::extract::Form<AlertFormData>,
) -> Result<Response, StatusCode> {
    let agent_id = if form.agent_id.as_deref() == Some("") || form.agent_id.is_none() {
        None
    } else {
        form.agent_id
    };

    let id = uuid::Uuid::new_v4().to_string();

    {
        let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        conn.execute(
            "INSERT INTO alert_rules (id, name, condition, agent_id, webhook_url, enabled, silence_minutes)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, 5)",
            params![id, form.condition, form.condition, agent_id, form.webhook_url.trim()],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    // Return success message; the hx-on::after-request in the template reloads the page
    Ok(Html(r#"<div class="msg-success">Alert rule created.</div>"#.to_string()).into_response())
}

#[derive(Debug, serde::Deserialize)]
pub struct AlertFormData {
    pub condition: String,
    pub agent_id: Option<String>,
    pub webhook_url: String,
}

/// DELETE /alerts/:id — delete alert (HTMX)
pub async fn delete_alert_html(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    conn.execute("DELETE FROM alert_rules WHERE id = ?1", params![id])
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Return empty to remove the row via hx-swap="outerHTML"
    Ok(Html(String::new()).into_response())
}

/// GET /settings — settings page
pub async fn settings_page(Extension(db): Extension<DbPool>) -> Result<Response, StatusCode> {
    let keys = query_api_keys(&db);

    let template = SettingsTemplate {
        active_nav: "settings".to_string(),
        keys,
        new_key: None,
    };
    Ok(template.into_response())
}

fn query_api_keys(db: &DbPool) -> Vec<ApiKeyView> {
    let conn = db.get().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT id, label, key_prefix, created_at, last_used_at, revoked
             FROM api_keys WHERE revoked = 0 ORDER BY created_at DESC",
        )
        .unwrap();

    stmt.query_map([], |row| {
        Ok(ApiKeyRow {
            id: row.get(0)?,
            label: row.get(1)?,
            key_prefix: row.get(2)?,
            created_at: row.get(3)?,
            last_used_at: row.get(4)?,
            revoked: row.get::<_, i64>(5)? != 0,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .map(|r| r.to_view())
    .collect()
}

/// GET /settings/keys/new-form — HTMX partial for key creation form
pub async fn settings_key_new_form() -> Result<Response, StatusCode> {
    let template = SettingsKeyFormTemplate {};
    Ok(template.into_response())
}

/// GET /settings/keys/cancel-form — returns empty to clear form
pub async fn settings_key_cancel_form() -> Html<String> {
    Html(String::new())
}

/// POST /settings/keys — generate new API key (HTMX form handler)
pub async fn create_key_form(
    Extension(db): Extension<DbPool>,
    axum::extract::Form(form): axum::extract::Form<CreateKeyFormData>,
) -> Result<Response, StatusCode> {
    let raw_key = crate::auth::generate_raw_key();
    let key_hash = crate::auth::hash_key(&raw_key);
    let key_prefix = raw_key[raw_key.len().saturating_sub(4)..].to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let label = if form.label.trim().is_empty() {
        "unnamed".to_string()
    } else {
        form.label.trim().to_string()
    };

    {
        let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        conn.execute(
            "INSERT INTO api_keys (id, label, key_hash, key_prefix) VALUES (?1, ?2, ?3, ?4)",
            params![id, label, key_hash, key_prefix],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    // Return the key display and trigger page reload
    let html = format!(
        r#"<div class="key-display">
  <div>Your new API key:</div>
  <code>{}</code>
  <p>Copy this key now. It will not be shown again.</p>
</div>"#,
        raw_key
    );

    Ok(Html(html).into_response())
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateKeyFormData {
    pub label: String,
}

/// DELETE /settings/keys/:id — revoke key (HTMX)
pub async fn revoke_key_html(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let active_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM api_keys WHERE revoked = 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if active_count <= 1 {
        return Ok(Html(r#"<tr><td colspan="4" class="msg-error">Cannot revoke the last active API key.</td></tr>"#.to_string()).into_response());
    }

    conn.execute("UPDATE api_keys SET revoked = 1 WHERE id = ?1", params![id])
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(String::new()).into_response())
}

// ─── Errors page handler ───

/// GET /errors — global errors page
pub async fn errors_page(
    Extension(db): Extension<DbPool>,
    Query(params): Query<PaginationParams>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(50);

    let total_errors: i64 = conn
        .query_row("SELECT COUNT(*) FROM errors", [], |row| row.get(0))
        .unwrap_or(0);

    let mut stmt = conn
        .prepare(
            "SELECT a.id, a.name, e.severity, e.message, e.created_at
             FROM errors e JOIN agents a ON e.agent_id = a.id
             ORDER BY e.created_at DESC LIMIT ?1 OFFSET ?2",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let errors: Vec<ErrorView> = stmt
        .query_map(params![limit + 1, offset], |row| {
            Ok(ErrorView {
                agent_id: row.get(0)?,
                agent_name: row.get(1)?,
                severity: row.get(2)?,
                message: row.get(3)?,
                timestamp: row
                    .get::<_, String>(4)
                    .map(|ts| crate::models::relative_time(&ts))
                    .unwrap_or_else(|_| "unknown".to_string()),
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter_map(|r| r.ok())
        .collect();

    let has_more = errors.len() as i64 > limit;
    let errors: Vec<ErrorView> = errors.into_iter().take(limit as usize).collect();

    let template = ErrorsTemplate {
        active_nav: "errors".to_string(),
        errors,
        total_errors,
        has_more,
        next_offset: offset + limit,
    };
    Ok(template.into_response())
}

// ─── Embeddings handlers ───

#[derive(Debug, Deserialize)]
pub struct EmbeddingsSearchParams {
    pub q: Option<String>,
    pub source_type: Option<String>,
}

/// GET /embeddings — embeddings page with sources list
pub async fn embeddings_page(Extension(db): Extension<DbPool>) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut stmt = conn
        .prepare(
            "SELECT source_type, source_name, COUNT(*) as chunk_count, MAX(updated_at) as last_updated
             FROM embedding_documents GROUP BY source_type, source_name ORDER BY last_updated DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let sources: Vec<EmbeddingSourceView> = stmt
        .query_map([], |row| {
            Ok(EmbeddingSourceView {
                source_type: row.get(0)?,
                source_name: row.get(1)?,
                chunk_count: row.get(2)?,
                last_updated: row
                    .get::<_, String>(3)
                    .map(|ts| crate::models::relative_time(&ts))
                    .unwrap_or_else(|_| "unknown".to_string()),
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter_map(|r| r.ok())
        .collect();

    let template = EmbeddingsTemplate {
        active_nav: "embeddings".to_string(),
        sources,
    };
    Ok(template.into_response())
}

/// GET /embeddings/search — HTMX partial returning search results (uses vector search via Ollama)
pub async fn embeddings_search(
    Extension(db): Extension<DbPool>,
    Query(params): Query<EmbeddingsSearchParams>,
) -> Result<Response, StatusCode> {
    let query = params.q.unwrap_or_default();
    if query.trim().is_empty() {
        let template = EmbeddingsSearchResultsTemplate {
            results: vec![],
            query,
        };
        return Ok(template.into_response());
    }

    let source_type_filter = params.source_type.filter(|s| !s.is_empty());

    let search_results =
        crate::embeddings::search(&db, &query, source_type_filter.as_deref(), 10).await;

    let results = match search_results {
        Ok(sr) => sr
            .into_iter()
            .map(|r| {
                let truncated = if r.content.len() > 200 {
                    format!("{}...", &r.content[..200])
                } else {
                    r.content
                };
                let similarity = (1.0 / (1.0 + r.distance)) * 100.0;
                SearchResultView {
                    heading: r.heading,
                    content: truncated,
                    source_name: r.source_name,
                    source_type: r.source_type,
                    distance: format!("{:.1}% match", similarity),
                }
            })
            .collect(),
        Err(e) => {
            tracing::warn!("Vector search failed, falling back to text: {}", e);
            text_search_fallback(&db, &query, source_type_filter.as_deref())?
        }
    };

    let template = EmbeddingsSearchResultsTemplate { results, query };
    Ok(template.into_response())
}

fn text_search_fallback(
    db: &DbPool,
    query: &str,
    source_type: Option<&str>,
) -> Result<Vec<SearchResultView>, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let results: Vec<SearchResultView> = if let Some(st) = source_type {
        let mut stmt = conn
            .prepare(
                "SELECT source_name, source_type, heading, content FROM embedding_documents
                 WHERE source_type = ?1 AND (heading LIKE '%' || ?2 || '%' OR content LIKE '%' || ?2 || '%')
                 LIMIT 20",
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let rows: Vec<SearchResultView> = stmt
            .query_map(params![st, query], |row| {
                let content: String = row.get(3)?;
                let truncated = if content.len() > 200 {
                    format!("{}...", &content[..200])
                } else {
                    content
                };
                Ok(SearchResultView {
                    source_name: row.get(0)?,
                    source_type: row.get(1)?,
                    heading: row.get(2)?,
                    content: truncated,
                    distance: "text match".to_string(),
                })
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .filter_map(|r| r.ok())
            .collect();
        rows
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT source_name, source_type, heading, content FROM embedding_documents
                 WHERE heading LIKE '%' || ?1 || '%' OR content LIKE '%' || ?1 || '%'
                 LIMIT 20",
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let rows: Vec<SearchResultView> = stmt
            .query_map(params![query], |row| {
                let content: String = row.get(3)?;
                let truncated = if content.len() > 200 {
                    format!("{}...", &content[..200])
                } else {
                    content
                };
                Ok(SearchResultView {
                    source_name: row.get(0)?,
                    source_type: row.get(1)?,
                    heading: row.get(2)?,
                    content: truncated,
                    distance: "text match".to_string(),
                })
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .filter_map(|r| r.ok())
            .collect();
        rows
    };

    Ok(results)
}

/// DELETE /embeddings/sources/:name — delete all embeddings for a source
pub async fn delete_embedding_source_html(
    Extension(db): Extension<DbPool>,
    Path(name): Path<String>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let doc_ids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM embedding_documents WHERE source_name = ?1")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let ids = stmt
            .query_map(params![name], |row| row.get(0))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .filter_map(|r| r.ok())
            .collect();
        ids
    };

    for doc_id in &doc_ids {
        conn.execute(
            "DELETE FROM vec_embeddings WHERE document_id = ?1",
            params![doc_id],
        )
        .ok();
    }

    conn.execute(
        "DELETE FROM embedding_documents WHERE source_name = ?1",
        params![name],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(String::new()).into_response())
}

#[derive(Debug, Deserialize)]
pub struct SetStatusFormData {
    pub state: String,
}

/// POST /agents/:id/set-status — set agent status from UI (HTMX)
pub async fn set_agent_status_html(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
    axum::extract::Form(form): axum::extract::Form<SetStatusFormData>,
) -> Result<Response, StatusCode> {
    if !crate::models::is_valid_state(&form.state) {
        return Ok(
            Html(r#"<div class="msg-error">Invalid status.</div>"#.to_string()).into_response(),
        );
    }

    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let agent_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM agents WHERE id = ?1",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !agent_exists {
        return Ok(
            Html(r#"<div class="msg-error">Agent not found.</div>"#.to_string()).into_response(),
        );
    }

    let status_message = format!("Status manually set to {}", form.state);

    let status_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, ?3, ?4)",
        params![status_id, id, form.state, status_message],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let update_error = if crate::models::is_error_state(&form.state) {
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
        params![form.state, status_message, id],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(format!(
        r#"<div class="msg-success">Agent status set to <strong>{}</strong>.</div>"#,
        form.state
    ))
    .into_response())
}

/// POST /agents/:id/stop — stop agent from UI (HTMX)
pub async fn stop_agent_html(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let current_state: String = conn
        .query_row(
            "SELECT current_state FROM agents WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if !crate::models::is_stoppable_state(&current_state) {
        return Ok(Html(format!(
            r#"<div class="msg-error">Agent is in '{}' state and cannot be stopped.</div>"#,
            current_state
        ))
        .into_response());
    }

    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM stop_requests WHERE agent_id = ?1 AND status = 'pending'",
            params![id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if pending > 0 {
        return Ok(Html(
            r#"<div class="msg-error">Agent already has a pending stop request.</div>"#.to_string(),
        )
        .into_response());
    }

    let stop_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO stop_requests (id, agent_id, requested_by, reason) VALUES (?1, ?2, 'operator', 'Stopped from dashboard')",
        params![stop_id, id],
    ).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let status_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, 'stopping', 'Stop requested by operator from dashboard')",
        params![status_id, id],
    ).ok();
    conn.execute(
        "UPDATE agents SET current_state = 'stopping', last_message = 'Stop requested by operator from dashboard', last_update_at = datetime('now') WHERE id = ?1",
        params![id],
    ).ok();

    Ok(Html(r#"<div class="msg-success">Stop signal sent. Agent will stop on its next status check.</div>"#.to_string()).into_response())
}

/// POST /agents/:id/cancel-stop — cancel stop from UI (HTMX)
pub async fn cancel_stop_html(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let affected = conn.execute(
        "UPDATE stop_requests SET status = 'cancelled', resolved_at = datetime('now') WHERE agent_id = ?1 AND status = 'pending'",
        params![id],
    ).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if affected == 0 {
        return Ok(Html(
            r#"<div class="msg-error">No pending stop request found.</div>"#.to_string(),
        )
        .into_response());
    }

    let prev_state: String = conn
        .query_row(
            "SELECT state FROM status_updates WHERE agent_id = ?1 AND state != 'stopping' ORDER BY created_at DESC LIMIT 1",
            params![id], |row| row.get(0),
        )
        .unwrap_or_else(|_| "running".to_string());

    let status_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO status_updates (id, agent_id, state, message) VALUES (?1, ?2, ?3, 'Stop request cancelled by operator')",
        params![status_id, id, prev_state],
    ).ok();
    conn.execute(
        "UPDATE agents SET current_state = ?1, last_message = 'Stop request cancelled', last_update_at = datetime('now') WHERE id = ?2",
        params![prev_state, id],
    ).ok();

    Ok(Html(format!(
        r#"<div class="msg-success">Stop cancelled. Agent reverted to <strong>{}</strong>.</div>"#,
        prev_state
    ))
    .into_response())
}

// ─── Login / Logout ───

/// GET /login
pub async fn login_page() -> Response {
    LoginTemplate { error: None }.into_response()
}

#[derive(Debug, Deserialize)]
pub struct LoginFormData {
    pub api_key: String,
}

/// POST /login — validate key, set cookie, redirect to dashboard
pub async fn login_submit(
    Extension(db): Extension<DbPool>,
    axum::extract::Form(form): axum::extract::Form<LoginFormData>,
) -> Response {
    let key = form.api_key.trim();

    if crate::auth::validate_key(&db, key) {
        let cookie = format!(
            "mcpolly_key={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800",
            key
        );
        http::Response::builder()
            .status(http::StatusCode::SEE_OTHER)
            .header(http::header::LOCATION, "/")
            .header(http::header::SET_COOKIE, cookie)
            .body(axum::body::Body::empty())
            .unwrap()
    } else {
        LoginTemplate {
            error: Some("Invalid API key.".to_string()),
        }
        .into_response()
    }
}

/// POST /logout — clear cookie, redirect to login
pub async fn logout() -> Response {
    http::Response::builder()
        .status(http::StatusCode::SEE_OTHER)
        .header(http::header::LOCATION, "/login")
        .header(
            http::header::SET_COOKIE,
            "mcpolly_key=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        )
        .body(axum::body::Body::empty())
        .unwrap()
}
