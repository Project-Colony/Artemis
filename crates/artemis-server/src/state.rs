use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore};

use crate::db::DbPool;
use crate::ws::ConnectionMap;

#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    pub connections: ConnectionMap,
    /// Held while a sign-in or a closing socket decides whether the user
    /// comes online or goes offline, and stores and announces it.
    // ponytail: one lock for every user's sign-in and sign-out, held over
    // three queries. Per-user locks if sign-ins ever queue behind it.
    pub presence: Arc<Mutex<()>>,
    /// A permit for each socket that has not signed in yet.
    pub unauthenticated: Arc<Semaphore>,
    /// The relay's OAuth app on GitHub, which signs users in.
    pub github: artemis_auth::GitHub,
}
