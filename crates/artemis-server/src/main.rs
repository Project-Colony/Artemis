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
        // The OAuth handlers build the redirect URL from this on every login,
        // and the state cookie is Secure only for https. A value without its
        // scheme, such as localhost:3000, still parses as a URL, so check the
        // scheme and host once here rather than failing on each login.
        let web_url = oauth2::url::Url::parse(&base_url)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
        anyhow::ensure!(
            web_url,
            "BASE_URL {base_url:?} is not an http:// or https:// URL"
        );

        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind_addr: var("BIND_ADDR").unwrap_or_else(|| "127.0.0.1:3000".to_string()),
            base_url,
            github_client_id: required("GITHUB_CLIENT_ID")?,
            github_client_secret: required("GITHUB_CLIENT_SECRET")?,
        })
    }
}

/// Describes a .env error without the offending line. dotenvy's own message
/// prints that line in full, and after an unclosed quote every line up to the
/// end of the file, which can include the database password or the GitHub
/// secret.
fn dotenv_error(e: dotenvy::Error) -> anyhow::Error {
    let dotenvy::Error::LineParse(line, _) = &e else {
        return anyhow::Error::new(e).context("cannot read .env");
    };
    let line = line.trim_start();
    let line = line.strip_prefix("export ").unwrap_or(line);
    let key_end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(line.len());
    let (key, rest) = line.split_at(key_end);
    if key.is_empty() || !rest.trim_start().starts_with('=') {
        anyhow::anyhow!("cannot read .env: a line is not of the form KEY=value")
    } else {
        anyhow::anyhow!(
            "cannot read .env: the value of {key} does not parse; quote values that contain spaces, quotes, backslashes or $"
        )
    }
}

/// Every route the relay serves.
fn router(state: AppState) -> Router {
    Router::new()
        .merge(routes::api_routes())
        .merge(ws::ws_routes())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Values already in the environment win over the ones in .env.
    if let Err(e) = dotenvy::dotenv() {
        if !e.not_found() {
            return Err(dotenv_error(e));
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_lookup(|name| std::env::var(name).ok())?;

    let pool = db::connect(&config.database_url).await?;
    db::prepare(&pool).await?;
    tracing::info!("Database migrations applied");

    let app_state = AppState {
        db: pool,
        connections: ws::new_connection_map(),
        presence: Default::default(),
        github_client_id: config.github_client_id,
        github_client_secret: config.github_client_secret,
        base_url: config.base_url,
    };

    let app = router(app_state);

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

        // The first two still parse as URLs, with the host name as the scheme.
        for scheme_less in [
            "localhost:3000",
            "relay.example.com:3000",
            "ftp://relay.example",
        ] {
            env.pop();
            env.push(("BASE_URL", scheme_less));
            let err = config_from(&env).err().unwrap().to_string();
            assert!(err.contains("http://"), "{err}");
        }

        env.pop();
        env.push(("BASE_URL", "https://relay.example/"));
        assert_eq!(config_from(&env).unwrap().base_url, "https://relay.example");
    }

    #[test]
    fn unparsable_env_line_names_its_key_but_not_its_value() {
        let env_file = "DATABASE_URL=postgres://artemis:pa'ss@localhost/artemis\nGITHUB_CLIENT_SECRET=s3cret\n";
        let e = dotenvy::from_read_iter(env_file.as_bytes())
            .find_map(Result::err)
            .unwrap();
        let err = format!("{:#}", dotenv_error(e));
        assert!(err.contains("DATABASE_URL"), "{err}");
        assert!(!err.contains("pa'ss") && !err.contains("s3cret"), "{err}");

        let e = dotenvy::from_read_iter("s3cret\n".as_bytes())
            .find_map(Result::err)
            .unwrap();
        let err = format!("{:#}", dotenv_error(e));
        assert!(!err.contains("s3cret"), "{err}");
    }
}
