use askama::Template;
use axum::{
    extract::Extension,
    extract::{Path, Query},
    http::{self, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use rusqlite::params;
use serde::Deserialize;

use crate::db::DbPool;
use crate::models::*;

// ─── Cookie helpers ───

fn has_setup_cookie(headers: &HeaderMap) -> bool {
    headers
        .get("cookie")
        .and_then(|c| c.to_str().ok())
        .map(|s| s.contains("mcpolly_setup_complete"))
        .unwrap_or(false)
}

fn extract_cookie_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("cookie")
        .and_then(|c| c.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .filter_map(|part| part.trim().strip_prefix("mcpolly_key="))
                .next()
                .map(|v| v.trim().to_string())
        })
}

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
    pub reset_success: bool,
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

#[allow(dead_code)]
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

// ─── Setup wizard ───

#[derive(Template)]
#[template(path = "setup.html")]
pub struct SetupTemplate {
    pub active_nav: String,
    pub api_key: String,
    pub has_agents: bool,
    pub cursor_config: String,
    pub claude_config: String,
}

// ─── Knowledge page (renamed from Embeddings) ───

#[derive(Debug, Clone)]
pub struct KnowledgeSearchResultView {
    pub heading: String,
    pub content: String,
    pub source_name: String,
    pub source_type: String,
    pub relevance_pct: f64,
}

#[derive(Template)]
#[template(path = "knowledge.html")]
pub struct KnowledgeTemplate {
    pub active_nav: String,
    pub sources: Vec<EmbeddingSourceView>,
}

#[derive(Template)]
#[template(path = "knowledge_search_results.html")]
pub struct KnowledgeSearchResultsTemplate {
    pub results: Vec<KnowledgeSearchResultView>,
    pub query: String,
}

// ─── Agent tab partials ───

#[derive(Template)]
#[template(path = "partials/agent_tab_errors.html")]
pub struct AgentTabErrorsTemplate {
    pub errors: Vec<ErrorView>,
}

#[derive(Template)]
#[template(path = "partials/agent_tab_info.html")]
pub struct AgentTabInfoTemplate {
    pub agent: AgentView,
}

// ─── Search params (command bar) ───

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
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
                UNION ALL
                SELECT 'tool_call' as entry_type, status as state,
                    tool_name || COALESCE(' — ' || input_summary, '') || COALESCE(' [' || duration_ms || 'ms]', '') as message,
                    created_at
                FROM tool_calls WHERE agent_id = ?1
            ) ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
        )
        .unwrap();

    stmt.query_map(params![agent_id, limit, offset], |row| {
        let entry_type: String = row.get(0)?;
        let state_val: String = row.get(1)?;
        let state = if entry_type == "status" || entry_type == "tool_call" {
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
pub async fn dashboard(
    headers: HeaderMap,
    Extension(db): Extension<DbPool>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = query_agent_rows(&conn);

    if !has_setup_cookie(&headers) {
        return Ok(Redirect::to("/setup").into_response());
    }

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
        r#"<span class="health-strip-item">{} agents</span>
<span class="health-strip-item health-strip-running">&#9679; {} running</span>
<span class="health-strip-item health-strip-errored">&#9679; {} errored</span>
<span class="health-strip-item health-strip-offline">&#9675; {} offline</span>"#,
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
            "SELECT ar.id, ar.condition, ar.agent_id, ar.webhook_url, ar.channel_type, ar.enabled, ar.created_at, a.name
             FROM alert_rules ar LEFT JOIN agents a ON ar.agent_id = a.id
             ORDER BY ar.created_at DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let alerts: Vec<AlertView> = rules_stmt
        .query_map([], |row| {
            Ok(AlertView {
                id: row.get(0)?,
                condition: row.get(1)?,
                agent_name: row.get(7)?,
                webhook_url: row.get(3)?,
                channel_type: row
                    .get::<_, String>(4)
                    .unwrap_or_else(|_| "discord".to_string()),
                active: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
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

    let channel_type = if ["discord", "slack", "generic"].contains(&form.channel_type.as_str()) {
        form.channel_type
    } else {
        "discord".to_string()
    };

    {
        let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        conn.execute(
            "INSERT INTO alert_rules (id, name, condition, agent_id, webhook_url, channel_type, enabled, silence_minutes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 5)",
            params![id, form.condition, form.condition, agent_id, form.webhook_url.trim(), channel_type],
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
    #[serde(default = "default_form_channel_type")]
    pub channel_type: String,
}

fn default_form_channel_type() -> String {
    "discord".to_string()
}

/// DELETE /alerts/:id — delete alert (HTMX)
pub async fn delete_alert_html(
    Extension(db): Extension<DbPool>,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    conn.execute(
        "DELETE FROM alert_history WHERE alert_rule_id = ?1",
        params![id],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    conn.execute("DELETE FROM alert_rules WHERE id = ?1", params![id])
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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

/// GET /embeddings — embeddings page with sources list (legacy, replaced by /knowledge)
#[allow(dead_code)]
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

// ─── Setup wizard handler ───

/// GET /setup — first-run setup wizard (public route, handles its own auth)
pub async fn setup_page(
    headers: HeaderMap,
    Extension(db): Extension<DbPool>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // If there's a pending setup key (first-run or post-reset), consume it
    // and auto-authenticate the user by setting the session cookie.
    if let Some(pending_key) = crate::db::take_pending_setup_key() {
        let agent_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
            .unwrap_or(0);

        let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
        let host =
            std::env::var("MCPOLLY_HOST").unwrap_or_else(|_| format!("http://localhost:{}", port));

        let cursor_config = format!(
            "{{\n  \"mcpServers\": {{\n    \"mcpolly\": {{\n      \"url\": \"{}/mcp\",\n      \"headers\": {{\n        \"Authorization\": \"Bearer {}\"\n      }}\n    }}\n  }}\n}}",
            host, pending_key
        );
        let claude_config = cursor_config.clone();

        let template = SetupTemplate {
            active_nav: String::new(),
            api_key: pending_key.clone(),
            has_agents: agent_count > 0,
            cursor_config,
            claude_config,
        };

        let html = template.into_response();
        let (mut parts, body) = html.into_parts();
        let cookie = format!(
            "mcpolly_key={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800",
            pending_key
        );
        parts
            .headers
            .insert(http::header::SET_COOKIE, cookie.parse().unwrap());
        return Ok(http::Response::from_parts(parts, body));
    }

    // No pending key — require authentication
    let api_key = match extract_cookie_key(&headers) {
        Some(key) if crate::auth::validate_key(&db, &key) => key,
        _ => return Ok(Redirect::to("/login").into_response()),
    };

    let agent_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
        .unwrap_or(0);
    let has_agents = agent_count > 0;

    if has_agents && has_setup_cookie(&headers) {
        return Ok(Redirect::to("/").into_response());
    }

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let host =
        std::env::var("MCPOLLY_HOST").unwrap_or_else(|_| format!("http://localhost:{}", port));

    let cursor_config = format!(
        "{{\n  \"mcpServers\": {{\n    \"mcpolly\": {{\n      \"url\": \"{}/mcp\",\n      \"headers\": {{\n        \"Authorization\": \"Bearer {}\"\n      }}\n    }}\n  }}\n}}",
        host, api_key
    );
    let claude_config = cursor_config.clone();

    let template = SetupTemplate {
        active_nav: String::new(),
        api_key,
        has_agents,
        cursor_config,
        claude_config,
    };
    Ok(template.into_response())
}

// ─── Command bar search handlers ───

/// GET /search/agents?q= — command bar agent name search
pub async fn search_agents(
    Extension(db): Extension<DbPool>,
    Query(params): Query<SearchParams>,
) -> Result<Html<String>, StatusCode> {
    let query = params.q.unwrap_or_default();
    if query.trim().is_empty() {
        return Ok(Html(String::new()));
    }

    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, current_state, last_update_at FROM agents
             WHERE name LIKE '%' || ?1 || '%' ORDER BY last_update_at DESC NULLS LAST LIMIT 5",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut html = String::new();
    let rows = stmt
        .query_map(params![query], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    for row in rows.filter_map(|r| r.ok()) {
        let (id, name, status, last_update) = row;
        let last_seen = last_update
            .as_deref()
            .map(crate::models::relative_time)
            .unwrap_or_else(|| "never".to_string());
        html.push_str(&format!(
            r#"<a href="/agents/{}" class="search-result">
  <span class="search-result-name">{}</span>
  <span class="badge badge-{}">{}</span>
  <span class="search-result-meta">{}</span>
</a>"#,
            id, name, status, status, last_seen
        ));
    }

    if html.is_empty() {
        html = r#"<div class="search-empty">No agents found</div>"#.to_string();
    }

    Ok(Html(html))
}

/// GET /search/errors?q= — command bar error message search
pub async fn search_errors(
    Extension(db): Extension<DbPool>,
    Query(params): Query<SearchParams>,
) -> Result<Html<String>, StatusCode> {
    let query = params.q.unwrap_or_default();
    if query.trim().is_empty() {
        return Ok(Html(String::new()));
    }

    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = conn
        .prepare(
            "SELECT a.name, e.message, e.created_at FROM errors e
             JOIN agents a ON e.agent_id = a.id
             WHERE e.message LIKE '%' || ?1 || '%'
             ORDER BY e.created_at DESC LIMIT 5",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut html = String::new();
    let rows = stmt
        .query_map(params![query], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    for row in rows.filter_map(|r| r.ok()) {
        let (agent_name, message, timestamp) = row;
        let time_str = crate::models::relative_time(&timestamp);
        let truncated_msg = if message.len() > 80 {
            format!("{}...", &message[..80])
        } else {
            message
        };
        html.push_str(&format!(
            r#"<div class="search-result">
  <span class="search-result-name">{}</span>
  <span class="search-result-detail">{}</span>
  <span class="search-result-meta">{}</span>
</div>"#,
            agent_name, truncated_msg, time_str
        ));
    }

    if html.is_empty() {
        html = r#"<div class="search-empty">No errors found</div>"#.to_string();
    }

    Ok(Html(html))
}

/// GET /search/knowledge?q= — command bar semantic search
pub async fn search_knowledge(
    Extension(db): Extension<DbPool>,
    Query(params): Query<SearchParams>,
) -> Result<Html<String>, StatusCode> {
    let query = params.q.unwrap_or_default();
    if query.trim().is_empty() {
        return Ok(Html(String::new()));
    }

    let search_results = crate::embeddings::search(&db, &query, None, 3).await;

    let mut html = String::new();
    match search_results {
        Ok(results) => {
            for r in results {
                let relevance_pct = ((2.0 - r.distance) / 2.0 * 100.0).max(0.0);
                html.push_str(&format!(
                    r#"<div class="search-result">
  <span class="search-result-name">{}</span>
  <span class="badge badge-info">{}</span>
  <span class="search-result-meta">{:.0}% relevant</span>
</div>"#,
                    r.heading, r.source_name, relevance_pct
                ));
            }
        }
        Err(_) => {
            html = r#"<div class="search-empty">Search unavailable</div>"#.to_string();
        }
    }

    if html.is_empty() {
        html = r#"<div class="search-empty">No results found</div>"#.to_string();
    }

    Ok(Html(html))
}

// ─── Agent tab handlers ───

/// GET /agents/:id/tab/:tab — tab content for agent detail
pub async fn agent_tab(
    Extension(db): Extension<DbPool>,
    Path((id, tab)): Path<(String, String)>,
) -> Result<Response, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match tab.as_str() {
        "activity" => {
            let limit: i64 = 50;
            let activities = query_activity(&conn, &id, limit + 1, 0);
            let has_more = activities.len() as i64 > limit;
            let entries: Vec<ActivityEntry> = activities.into_iter().take(limit as usize).collect();

            let template = ActivityFragmentTemplate {
                entries,
                agent_id: id,
                next_offset: limit,
                has_more,
            };
            Ok(template.into_response())
        }
        "errors" => {
            let mut stmt = conn
                .prepare(
                    "SELECT a.id, a.name, e.severity, e.message, e.created_at
                     FROM errors e JOIN agents a ON e.agent_id = a.id
                     WHERE e.agent_id = ?1
                     ORDER BY e.created_at DESC LIMIT 50",
                )
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let errors: Vec<ErrorView> = stmt
                .query_map(params![id], |row| {
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

            let template = AgentTabErrorsTemplate { errors };
            Ok(template.into_response())
        }
        "info" => {
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
            let template = AgentTabInfoTemplate { agent };
            Ok(template.into_response())
        }
        _ => Err(StatusCode::NOT_FOUND),
    }
}

// ─── Knowledge page handlers ───

/// GET /knowledge — knowledge page (renamed from /embeddings)
pub async fn knowledge_page(Extension(db): Extension<DbPool>) -> Result<Response, StatusCode> {
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

    let template = KnowledgeTemplate {
        active_nav: "knowledge".to_string(),
        sources,
    };
    Ok(template.into_response())
}

/// GET /knowledge/search — knowledge search results partial (with relevance %)
pub async fn knowledge_search(
    Extension(db): Extension<DbPool>,
    Query(params): Query<EmbeddingsSearchParams>,
) -> Result<Response, StatusCode> {
    let query = params.q.unwrap_or_default();
    if query.trim().is_empty() {
        let template = KnowledgeSearchResultsTemplate {
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
                let relevance_pct = ((2.0 - r.distance) / 2.0 * 100.0).max(0.0);
                KnowledgeSearchResultView {
                    heading: r.heading,
                    content: truncated,
                    source_name: r.source_name,
                    source_type: r.source_type,
                    relevance_pct,
                }
            })
            .collect(),
        Err(e) => {
            tracing::warn!("Vector search failed, falling back to text: {}", e);
            text_search_fallback_knowledge(&db, &query, source_type_filter.as_deref())?
        }
    };

    let template = KnowledgeSearchResultsTemplate { results, query };
    Ok(template.into_response())
}

fn text_search_fallback_knowledge(
    db: &DbPool,
    query: &str,
    source_type: Option<&str>,
) -> Result<Vec<KnowledgeSearchResultView>, StatusCode> {
    let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (sql, use_source_type) = if source_type.is_some() {
        (
            "SELECT source_name, source_type, heading, content FROM embedding_documents
             WHERE source_type = ?1 AND (heading LIKE '%' || ?2 || '%' OR content LIKE '%' || ?2 || '%')
             LIMIT 20",
            true,
        )
    } else {
        (
            "SELECT source_name, source_type, heading, content FROM embedding_documents
             WHERE heading LIKE '%' || ?1 || '%' OR content LIKE '%' || ?1 || '%'
             LIMIT 20",
            false,
        )
    };

    let mut stmt = conn
        .prepare(sql)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let map_row = |row: &rusqlite::Row| -> rusqlite::Result<KnowledgeSearchResultView> {
        let content: String = row.get(3)?;
        let truncated = if content.len() > 200 {
            format!("{}...", &content[..200])
        } else {
            content
        };
        Ok(KnowledgeSearchResultView {
            source_name: row.get(0)?,
            source_type: row.get(1)?,
            heading: row.get(2)?,
            content: truncated,
            relevance_pct: 0.0,
        })
    };

    let results: Vec<KnowledgeSearchResultView> = if use_source_type {
        stmt.query_map(params![source_type.unwrap_or(""), query], map_row)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .filter_map(|r| r.ok())
            .collect()
    } else {
        stmt.query_map(params![query], map_row)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .filter_map(|r| r.ok())
            .collect()
    };

    Ok(results)
}

/// GET /search?q= — unified command bar search across agents, errors, knowledge
pub async fn unified_search(
    Extension(db): Extension<DbPool>,
    Query(params): Query<SearchParams>,
) -> Result<Html<String>, StatusCode> {
    let query = params.q.unwrap_or_default();
    if query.trim().is_empty() {
        return Ok(Html(String::new()));
    }

    let mut html = String::new();

    // Synchronous DB queries in a block so conn is dropped before .await
    {
        let conn = db.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // Agents section
        let mut stmt = conn
            .prepare(
                "SELECT id, name, current_state, last_update_at FROM agents
                 WHERE name LIKE '%' || ?1 || '%' ORDER BY last_update_at DESC NULLS LAST LIMIT 3",
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let agents: Vec<(String, String, String, Option<String>)> = stmt
            .query_map(params![query], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .filter_map(|r| r.ok())
            .collect();

        if !agents.is_empty() {
            html.push_str(r#"<div class="search-group-label">Agents</div>"#);
            for (id, name, status, last_update) in &agents {
                let last_seen = last_update
                    .as_deref()
                    .map(crate::models::relative_time)
                    .unwrap_or_else(|| "never".to_string());
                html.push_str(&format!(
                    r#"<a href="/agents/{}" class="search-result"><span class="search-result-name">{}</span><span class="badge badge-{}">{}</span><span class="search-result-meta">{}</span></a>"#,
                    id, name, status, status, last_seen
                ));
            }
        }

        // Errors section
        let mut stmt = conn
            .prepare(
                "SELECT a.id, a.name, e.message, e.created_at FROM errors e
                 JOIN agents a ON e.agent_id = a.id
                 WHERE e.message LIKE '%' || ?1 || '%'
                 ORDER BY e.created_at DESC LIMIT 3",
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let errors: Vec<(String, String, String, String)> = stmt
            .query_map(params![query], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .filter_map(|r| r.ok())
            .collect();

        if !errors.is_empty() {
            html.push_str(r#"<div class="search-group-label">Errors</div>"#);
            for (agent_id, agent_name, message, timestamp) in &errors {
                let time_str = crate::models::relative_time(timestamp);
                let truncated = if message.len() > 60 {
                    format!("{}...", &message[..60])
                } else {
                    message.clone()
                };
                html.push_str(&format!(
                    r#"<a href="/agents/{}" class="search-result"><span class="search-result-name">{}</span><span class="search-result-detail">{}</span><span class="search-result-meta">{}</span></a>"#,
                    agent_id, agent_name, truncated, time_str
                ));
            }
        }
    } // conn dropped here

    // Knowledge section — async, needs conn dropped first
    if query.len() > 3 {
        if let Ok(results) = crate::embeddings::search(&db, &query, None, 3).await {
            if !results.is_empty() {
                html.push_str(r#"<div class="search-group-label">Knowledge</div>"#);
                for r in results {
                    let relevance_pct = ((2.0 - r.distance) / 2.0 * 100.0).max(0.0);
                    html.push_str(&format!(
                        r#"<div class="search-result"><span class="search-result-name">{}</span><span class="badge badge-info">{}</span><span class="search-result-meta">{:.0}%</span></div>"#,
                        r.heading, r.source_name, relevance_pct
                    ));
                }
            }
        }
    }

    if html.is_empty() {
        html = r#"<div class="search-empty">No results found</div>"#.to_string();
    }

    Ok(Html(html))
}

/// GET /settings/server-info — server info partial for settings page
pub async fn server_info_partial(
    Extension(db): Extension<DbPool>,
) -> Result<Html<String>, StatusCode> {
    let version = env!("CARGO_PKG_VERSION");
    let uptime = crate::api::get_uptime_string();

    let db_size = std::fs::metadata(
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "mcpolly.db".to_string()),
    )
    .map(|m| {
        let bytes = m.len();
        if bytes > 1_048_576 {
            format!("{:.1} MB", bytes as f64 / 1_048_576.0)
        } else {
            format!("{:.0} KB", bytes as f64 / 1024.0)
        }
    })
    .unwrap_or_else(|_| "unknown".to_string());

    let ollama_url =
        std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let ollama_model =
        std::env::var("OLLAMA_EMBEDDING_MODEL").unwrap_or_else(|_| "all-minilm".to_string());
    let _ = &db; // used for pool check if needed

    let ollama_connected = reqwest::Client::new()
        .head(&ollama_url)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .is_ok();

    let ollama_status = if ollama_connected {
        format!(
            r#"<span style="color:var(--status-running-fg)">● Connected</span> ({})"#,
            ollama_model
        )
    } else {
        r#"<span style="color:var(--status-offline-fg)">○ Disconnected</span>"#.to_string()
    };

    let html = format!(
        r#"<div class="card-header"><h2>Server</h2></div>
<dl style="margin:0">
  <div style="display:flex;gap:24px;flex-wrap:wrap">
    <div><dt class="text-muted" style="font-size:11px;text-transform:uppercase">Version</dt><dd style="font-weight:600">{}</dd></div>
    <div><dt class="text-muted" style="font-size:11px;text-transform:uppercase">Uptime</dt><dd style="font-weight:600">{}</dd></div>
    <div><dt class="text-muted" style="font-size:11px;text-transform:uppercase">Database</dt><dd style="font-weight:600">{}</dd></div>
    <div><dt class="text-muted" style="font-size:11px;text-transform:uppercase">Ollama</dt><dd style="font-weight:600">{}</dd></div>
  </div>
</dl>"#,
        version, uptime, db_size, ollama_status
    );

    Ok(Html(html))
}

/// GET /embeddings — redirect 301 to /knowledge
pub async fn embeddings_redirect() -> Response {
    Redirect::permanent("/knowledge").into_response()
}

// ─── Login / Logout ───

/// GET /login — if a pending setup key exists, skip login and go straight to setup
pub async fn login_page(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    if crate::db::has_pending_setup_key() {
        return Redirect::to("/setup").into_response();
    }

    LoginTemplate {
        error: None,
        reset_success: params.contains_key("reset"),
    }
    .into_response()
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
            reset_success: false,
        }
        .into_response()
    }
}

/// POST /reset-instance — revoke all keys, generate a new default, redirect to setup
pub async fn reset_instance(Extension(db): Extension<DbPool>) -> Response {
    let conn = db.get().expect("db connection");

    let revoked: usize = conn
        .execute("UPDATE api_keys SET revoked = 1", [])
        .unwrap_or(0);
    tracing::warn!("Instance reset: revoked {} API key(s)", revoked);

    let new_key = crate::db::seed_default_key_if_empty(&conn);

    if new_key.is_none() {
        tracing::error!("Reset failed: could not generate a new key");
    }

    // Redirect straight to /setup — the pending key mechanism will
    // auto-authenticate the user and show them the new key.
    let mut builder = http::Response::builder()
        .status(http::StatusCode::SEE_OTHER)
        .header(http::header::LOCATION, "/setup");

    for cookie in [
        "mcpolly_key=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        "mcpolly_setup_complete=; Path=/; Max-Age=0",
    ] {
        builder = builder.header(http::header::SET_COOKIE, cookie);
    }

    builder.body(axum::body::Body::empty()).unwrap()
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
