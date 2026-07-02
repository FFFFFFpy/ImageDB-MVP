use crate::infrastructure::postgres::PostgresManager;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AppState {
    pub postgres_manager: Arc<Mutex<PostgresManager>>,
    pub fixture_dir: PathBuf,
}

impl AppState {
    pub fn new(app_data_dir: &std::path::Path, fixture_dir: PathBuf) -> Self {
        Self {
            postgres_manager: Arc::new(Mutex::new(PostgresManager::new(app_data_dir))),
            fixture_dir,
        }
    }
}
