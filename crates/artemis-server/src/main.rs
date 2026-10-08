mod db;
mod routes;
mod state;
mod ws;

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // Config from env vars
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://artemis:artemis@localhost:5432/artemis".to_string());
    let github_client_id = std::env::var("GITHUB_CLIENT_ID").unwrap_or_default();
    let github_client_secret = std::env::var("GITHUB_CLIENT_SECRET").unwrap_or_default();
    let base_url =
        std::env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:3000".to_string());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());

    // Connect to database
    let pool = db::connect(&database_url).await?;

    // Run migrations
    db::run_migrations(&pool).await?;

    let app_state = AppState {
        db: pool,
        connections: ws::new_connection_map(),
        github_client_id,
        github_client_secret,
        base_url,
    };

    let app = Router::new()
        .merge(routes::api_routes())
        .merge(ws::ws_routes())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(app_state);

    tracing::info!("Artemis server listening on {bind_addr}");

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
