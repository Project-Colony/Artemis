mod db;
mod routes;
mod state;
mod ws;

use anyhow::Context;
use axum::Router;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use state::AppState;

/// The relay's settings, read once at startup.
struct Config {
    database_url: String,
    bind_addr: String,
    base_url: String,
    github_client_id: String,
    github_client_secret: String,
}

impl Config {
    /// Reads every setting through `var`, which returns a variable's value or
    /// `None`. A missing or empty required setting stops the relay instead of
    /// falling back to a value the operator never chose.
    fn from_lookup(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let required = |name: &str| match var(name) {
            Some(value) if !value.trim().is_empty() => Ok(value),
            _ => anyhow::bail!(
                "{name} is missing or empty; copy .env.example to .env and fill it in"
            ),
        };

        let base_url = var("BASE_URL").unwrap_or_else(|| "http://localhost:3000".to_string());
        let base_url = base_url.trim_end_matches('/').to_string();
        // The OAuth handlers build this redirect URL on every login; check it
        // parses once here rather than failing on each request.
        oauth2::RedirectUrl::new(format!("{base_url}/auth/github/callback"))
            .with_context(|| format!("BASE_URL {base_url:?} is not an absolute URL"))?;

        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind_addr: var("BIND_ADDR").unwrap_or_else(|| "127.0.0.1:3000".to_string()),
            base_url,
            github_client_id: required("GITHUB_CLIENT_ID")?,
            github_client_secret: required("GITHUB_CLIENT_SECRET")?,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Values already in the environment win over the ones in .env.
    if let Err(e) = dotenvy::dotenv() {
        if !e.not_found() {
            return Err(e).context("cannot read .env");
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_lookup(|name| std::env::var(name).ok())?;

    let pool = db::connect(&config.database_url).await?;
    db::MIGRATOR.run(&pool).await?;
    tracing::info!("Database migrations applied");

    // Nobody is connected yet. Users who were online when the relay last
    // stopped would otherwise stay online for their friends.
    db::mark_all_users_offline(&pool).await?;

    let app_state = AppState {
        db: pool,
        connections: ws::new_connection_map(),
        github_client_id: config.github_client_id,
        github_client_secret: config.github_client_secret,
        base_url: config.base_url,
    };

    let app = Router::new()
        .merge(routes::api_routes())
        .merge(ws::ws_routes())
        .layer(TraceLayer::new_for_http())
        .with_state(app_state);

    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("cannot listen on BIND_ADDR {}", config.bind_addr))?;
    tracing::info!("Artemis server listening on {}", config.bind_addr);
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config_from(pairs: &[(&str, &str)]) -> anyhow::Result<Config> {
        let env: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|name| env.get(name).cloned())
    }

    const COMPLETE: [(&str, &str); 3] = [
        (
            "DATABASE_URL",
            "postgres://artemis:secret@localhost:5432/artemis",
        ),
        ("GITHUB_CLIENT_ID", "id"),
        ("GITHUB_CLIENT_SECRET", "secret"),
    ];

    #[test]
    fn missing_database_url_points_at_env_example() {
        let err = config_from(&COMPLETE[1..]).err().unwrap().to_string();
        assert!(err.contains("DATABASE_URL"), "{err}");
        assert!(err.contains(".env.example"), "{err}");
    }

    #[test]
    fn empty_github_client_id_is_refused() {
        let mut env = COMPLETE.to_vec();
        env[1].1 = "";
        let err = config_from(&env).err().unwrap().to_string();
        assert!(err.contains("GITHUB_CLIENT_ID"), "{err}");
    }

    #[test]
    fn defaults_bind_to_loopback() {
        let config = config_from(&COMPLETE).unwrap();
        assert_eq!(config.bind_addr, "127.0.0.1:3000");
        assert_eq!(config.base_url, "http://localhost:3000");
    }

    #[test]
    fn base_url_must_be_absolute() {
        let mut env = COMPLETE.to_vec();
        env.push(("BASE_URL", "/relay"));
        assert!(config_from(&env).is_err());

        env.pop();
        env.push(("BASE_URL", "https://relay.example/"));
        assert_eq!(config_from(&env).unwrap().base_url, "https://relay.example");
    }
}
