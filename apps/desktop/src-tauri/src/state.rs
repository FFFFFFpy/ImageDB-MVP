use crate::infrastructure::postgres::PostgresManager;
use crate::infrastructure::settings::SettingsStore;
use crate::services::DatabaseService;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AppState {
    pub postgres_manager: Arc<Mutex<PostgresManager>>,
    pub settings: Arc<Mutex<SettingsStore>>,
    pub database_service: DatabaseService,
    pub fixture_dir: PathBuf,
}

impl AppState {
    pub fn new(
        app_data_dir: &std::path::Path,
        fixture_dir: PathBuf,
    ) -> Result<Self, crate::error::AppError> {
        let postgres_manager = Arc::new(Mutex::new(PostgresManager::new(app_data_dir)));
        let settings = Arc::new(Mutex::new(SettingsStore::new(app_data_dir)?));
        let database_service = DatabaseService::new(postgres_manager.clone(), settings.clone());

        Ok(Self {
            postgres_manager,
            settings,
            database_service,
            fixture_dir,
        })
    }
}
