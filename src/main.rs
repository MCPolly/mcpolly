use axum::{
    middleware,
    routing::{delete, get, post, put},
    Extension, Router,
};
use rmcp::transport::{
    StreamableHttpServerConfig,
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
};
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod alerts;
mod api;
mod auth;
mod db;
mod embeddings;
mod mcp;
mod mcp_server;
mod models;
mod templates;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mcpolly=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "mcpolly.db".to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let db = db::init_db(&database_url);

    {
        let conn = db.get().expect("Failed to get connection for seeding");
        db::seed_default_key_if_empty(&conn);
    }

    let bg_db = db.clone();
    tokio::spawn(async move {
        alerts::silent_agent_checker(bg_db).await;
    });

    // JSON API routes — authenticated via Authorization header or X-API-Key
    let api_routes = Router::new()
        .route("/agents", get(api::list_agents))
        .route("/agents/register", post(mcp::register_agent))
        .route("/agents/:id", get(api::get_agent))
        .route("/agents/:id/activity", get(api::get_agent_activity))
        .route("/agents/:id/errors", get(api::get_agent_errors))
        .route("/agents/:id/status", put(api::set_agent_status))
        .route("/agents/:id/stop", post(api::stop_agent).delete(api::cancel_stop_agent).get(api::get_stop_status))
        .route("/status", post(mcp::post_status))
        .route("/errors", post(mcp::post_error))
        .route("/alerts", get(api::list_alerts).post(api::create_alert))
        .route("/alerts/history", get(api::list_alert_history))
        .route("/alerts/:id", delete(api::delete_alert))
        .route("/keys", get(api::list_api_keys).post(api::create_api_key))
        .route("/keys/:id", delete(api::revoke_api_key))
        .route("/embeddings/index", post(api::index_embeddings))
        .route("/embeddings/search", get(api::search_embeddings_api))
        .route("/embeddings/sources", get(api::list_embedding_sources))
        .route(
            "/embeddings/sources/:name",
            delete(api::delete_embedding_source),
        )
        .layer(middleware::from_fn(auth::require_api_key));

    // HTMX UI routes — authenticated via cookie, redirects to /login if missing
    let ui_routes = Router::new()
        .route("/", get(templates::dashboard))
        .route("/partials/agents", get(templates::agents_partial))
        .route("/agents/:id", get(templates::agent_detail))
        .route(
            "/agents/:id/activity",
            get(templates::agent_activity_fragment),
        )
        .route("/agents/:id/set-status", post(templates::set_agent_status_html))
        .route("/agents/:id/stop", post(templates::stop_agent_html))
        .route("/agents/:id/cancel-stop", post(templates::cancel_stop_html))
        .route(
            "/alerts",
            get(templates::alerts_page).post(templates::create_alert_form),
        )
        .route("/errors", get(templates::errors_page))
        .route("/partials/summary", get(templates::summary_partial))
        .route("/alerts/new-form", get(templates::alerts_new_form))
        .route("/alerts/cancel-form", get(templates::alerts_cancel_form))
        .route("/alerts/:id", delete(templates::delete_alert_html))
        .route("/settings", get(templates::settings_page))
        .route(
            "/settings/keys/new-form",
            get(templates::settings_key_new_form),
        )
        .route(
            "/settings/keys/cancel-form",
            get(templates::settings_key_cancel_form),
        )
        .route("/settings/keys", post(templates::create_key_form))
        .route("/settings/keys/:id", delete(templates::revoke_key_html))
        .route("/embeddings", get(templates::embeddings_page))
        .route("/embeddings/search", get(templates::embeddings_search))
        .route(
            "/embeddings/sources/:name",
            delete(templates::delete_embedding_source_html),
        )
        .layer(middleware::from_fn(auth::require_auth_or_redirect));

    // MCP JSON-RPC HTTP streaming endpoint — authenticated via API key
    let db_for_mcp = db.clone();
    let mcp_service: StreamableHttpService<mcp_server::McPollyHandler, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(mcp_server::McPollyHandler::new(db_for_mcp.clone())),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );
    let mcp_routes = Router::new()
        .nest_service("/mcp", mcp_service)
        .layer(middleware::from_fn(auth::require_api_key));

    // Public routes — no authentication
    let public_routes = Router::new()
        .route(
            "/login",
            get(templates::login_page).post(templates::login_submit),
        )
        .route("/logout", post(templates::logout))
        .route("/health", get(health_check));

    let app = Router::new()
        .nest("/api/v1", api_routes)
        .merge(mcp_routes)
        .merge(ui_routes)
        .merge(public_routes)
        .layer(Extension(db))
        .layer(CorsLayer::permissive());

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("MCPolly listening on http://{}", addr);
    tracing::info!("MCP HTTP endpoint available at http://{}/mcp", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health_check() -> &'static str {
    "ok"
}
