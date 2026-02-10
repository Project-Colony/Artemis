use crate::db::DbPool;
use crate::ws::ConnectionMap;

#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    pub connections: ConnectionMap,
    pub github_client_id: String,
    pub github_client_secret: String,
    pub base_url: String,
}
