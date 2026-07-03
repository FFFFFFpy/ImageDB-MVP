use crate::domain::import_state::{CommitProgress, ScanProgress};
use crate::infrastructure::postgres::PostgresManager;
use crate::infrastructure::settings::SettingsStore;
use crate::services::DatabaseService;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct ScanHandle {
    pub cancelled: Arc<AtomicBool>,
    pub task: tokio::task::JoinHandle<ScanProgress>,
}

pub struct ScanState {
    pub active: Option<ScanHandle>,
    pub progress_tracker: Arc<Mutex<ScanProgress>>,
}

impl Default for ScanState {
    fn default() -> Self {
        Self {
            active: None,
            progress_tracker: Arc::new(Mutex::new(ScanProgress::idle())),
        }
    }
}

pub struct CommitHandle {
    pub cancelled: Arc<AtomicBool>,
    pub task: tokio::task::JoinHandle<CommitProgress>,
}

pub struct CommitState {
    pub active: Option<CommitHandle>,
    pub progress_tracker: Arc<Mutex<CommitProgress>>,
}

impl Default for CommitState {
    fn default() -> Self {
        Self {
            active: None,
            progress_tracker: Arc::new(Mutex::new(CommitProgress::idle(""))),
        }
    }
}

pub struct AppState {
    pub postgres_manager: Arc<Mutex<PostgresManager>>,
    pub settings: Arc<Mutex<SettingsStore>>,
    pub database_service: DatabaseService,
    pub fixture_dir: PathBuf,
    pub scan_state: Arc<Mutex<ScanState>>,
    pub commit_state: Arc<Mutex<CommitState>>,
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
            scan_state: Arc::new(Mutex::new(ScanState::default())),
            commit_state: Arc::new(Mutex::new(CommitState::default())),
        })
    }
}
