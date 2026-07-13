use crate::domain::import_state::{CommitProgress, ScanProgress};
use crate::domain::ExternalMigrationProgress;
use crate::infrastructure::postgres::PostgresManager;
use crate::infrastructure::secrets::CredentialStore;
use crate::infrastructure::settings::SettingsStore;
use crate::services::DatabaseService;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CriticalTaskKind {
    Scan,
    Commit,
    Recovery,
    ExternalMigration,
}

impl CriticalTaskKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Commit => "commit",
            Self::Recovery => "recovery",
            Self::ExternalMigration => "external_migration",
        }
    }

    const fn display_name(self) -> &'static str {
        match self {
            Self::Scan => "import scan",
            Self::Commit => "import commit",
            Self::Recovery => "transaction recovery",
            Self::ExternalMigration => "external database migration",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriticalOperationKind {
    InitializeManagedDatabase,
    ProbeManagedDatabase,
    SwitchToManagedDatabase,
    InitializeExternalDatabase,
    ShutdownDatabase,
    UpdateCriticalSettings,
}

impl CriticalOperationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InitializeManagedDatabase => "initialize_managed_database",
            Self::ProbeManagedDatabase => "probe_managed_database",
            Self::SwitchToManagedDatabase => "switch_to_managed_database",
            Self::InitializeExternalDatabase => "initialize_external_database",
            Self::ShutdownDatabase => "shutdown_database",
            Self::UpdateCriticalSettings => "update_critical_settings",
        }
    }

    const fn action(self) -> &'static str {
        match self {
            Self::InitializeManagedDatabase => "initialize managed database",
            Self::ProbeManagedDatabase => "probe managed database",
            Self::SwitchToManagedDatabase => "switch database",
            Self::InitializeExternalDatabase => "initialize external database",
            Self::ShutdownDatabase => "stop database",
            Self::UpdateCriticalSettings => "change database or library settings",
        }
    }

    const fn display_name(self) -> &'static str {
        match self {
            Self::InitializeManagedDatabase => "managed database initialization",
            Self::ProbeManagedDatabase => "managed database probe",
            Self::SwitchToManagedDatabase => "database switch",
            Self::InitializeExternalDatabase => "external database initialization",
            Self::ShutdownDatabase => "database shutdown",
            Self::UpdateCriticalSettings => "database or library settings update",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriticalOperationGuardSnapshot {
    pub is_blocked: bool,
    pub blocking_reason: Option<String>,
    pub active_task_kinds: Vec<String>,
    pub active_operation: Option<String>,
}

#[derive(Debug, Default)]
struct CriticalOperationGuardInner {
    active_tasks: BTreeMap<CriticalTaskKind, usize>,
    active_operation: Option<CriticalOperationKind>,
}

/// Serializes long-running database/library tasks against lifecycle and
/// critical-settings mutations.
///
/// The registry mutex is held only while registering or releasing a lease; it
/// is never held across an `.await`. Long-running work owns an RAII lease, so
/// task completion (including panic/cancellation) releases the protection.
#[derive(Clone, Debug, Default)]
pub struct CriticalOperationGuard {
    inner: Arc<StdMutex<CriticalOperationGuardInner>>,
}

impl CriticalOperationGuard {
    pub fn begin_task(&self, kind: CriticalTaskKind) -> Result<CriticalTaskLease, String> {
        let mut inner = self.lock_inner();
        if let Some(operation) = inner.active_operation {
            return Err(format!(
                "cannot start {} while {} is running",
                kind.display_name(),
                operation.display_name()
            ));
        }

        let migration_is_active = inner
            .active_tasks
            .get(&CriticalTaskKind::ExternalMigration)
            .copied()
            .unwrap_or_default()
            > 0;
        if (kind == CriticalTaskKind::ExternalMigration && !inner.active_tasks.is_empty())
            || (kind != CriticalTaskKind::ExternalMigration && migration_is_active)
        {
            let blocker = if migration_is_active {
                CriticalTaskKind::ExternalMigration
            } else {
                *inner
                    .active_tasks
                    .keys()
                    .next()
                    .expect("non-empty active task map")
            };
            return Err(format!(
                "cannot start {} while {} is running",
                kind.display_name(),
                blocker.display_name()
            ));
        }

        *inner.active_tasks.entry(kind).or_default() += 1;
        Ok(CriticalTaskLease {
            guard: self.clone(),
            kind,
        })
    }

    pub fn begin_operation(
        &self,
        operation: CriticalOperationKind,
    ) -> Result<CriticalOperationLease, String> {
        let mut inner = self.lock_inner();
        if let Some(active_operation) = inner.active_operation {
            return Err(format!(
                "cannot {} while {} is running",
                operation.action(),
                active_operation.display_name()
            ));
        }
        if let Some((&kind, _)) = inner.active_tasks.iter().next() {
            return Err(format!(
                "cannot {} while {} is running",
                operation.action(),
                kind.display_name()
            ));
        }

        inner.active_operation = Some(operation);
        Ok(CriticalOperationLease {
            guard: self.clone(),
            operation,
        })
    }

    pub fn snapshot(&self) -> CriticalOperationGuardSnapshot {
        let inner = self.lock_inner();
        let active_task_kinds = inner
            .active_tasks
            .keys()
            .map(|kind| kind.as_str().to_string())
            .collect::<Vec<_>>();
        let active_operation = inner
            .active_operation
            .map(|operation| operation.as_str().to_string());
        let blocking_reason = if let Some(operation) = inner.active_operation {
            Some(format!(
                "Database and library settings are locked while {} is running",
                operation.display_name()
            ))
        } else if inner.active_tasks.len() == 1 {
            let kind = *inner
                .active_tasks
                .keys()
                .next()
                .expect("single active task kind");
            Some(format!(
                "Database and library settings are locked while {} is running",
                kind.display_name()
            ))
        } else if !inner.active_tasks.is_empty() {
            let tasks = inner
                .active_tasks
                .keys()
                .map(|kind| kind.display_name())
                .collect::<Vec<_>>()
                .join(", ");
            Some(format!(
                "Database and library settings are locked while these tasks are running: {tasks}"
            ))
        } else {
            None
        };

        CriticalOperationGuardSnapshot {
            is_blocked: blocking_reason.is_some(),
            blocking_reason,
            active_task_kinds,
            active_operation,
        }
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, CriticalOperationGuardInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
pub struct CriticalTaskLease {
    guard: CriticalOperationGuard,
    kind: CriticalTaskKind,
}

impl Drop for CriticalTaskLease {
    fn drop(&mut self) {
        let mut inner = self.guard.lock_inner();
        if let Some(count) = inner.active_tasks.get_mut(&self.kind) {
            *count -= 1;
            if *count == 0 {
                inner.active_tasks.remove(&self.kind);
            }
        }
    }
}

#[derive(Debug)]
pub struct CriticalOperationLease {
    guard: CriticalOperationGuard,
    operation: CriticalOperationKind,
}

impl Drop for CriticalOperationLease {
    fn drop(&mut self) {
        let mut inner = self.guard.lock_inner();
        if inner.active_operation == Some(self.operation) {
            inner.active_operation = None;
        }
    }
}

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

pub struct ExternalMigrationHandle {
    pub cancelled: Arc<AtomicBool>,
    pub task: tokio::task::JoinHandle<ExternalMigrationProgress>,
}

pub struct ExternalMigrationState {
    pub active: Option<ExternalMigrationHandle>,
    pub progress_tracker: Arc<Mutex<ExternalMigrationProgress>>,
}

impl Default for ExternalMigrationState {
    fn default() -> Self {
        Self {
            active: None,
            progress_tracker: Arc::new(Mutex::new(ExternalMigrationProgress::idle())),
        }
    }
}

pub struct AppState {
    pub app_data_dir: PathBuf,
    pub postgres_manager: Arc<Mutex<PostgresManager>>,
    pub settings: Arc<Mutex<SettingsStore>>,
    pub database_service: DatabaseService,
    pub fixture_dir: PathBuf,
    pub scan_state: Arc<Mutex<ScanState>>,
    pub commit_state: Arc<Mutex<CommitState>>,
    pub external_migration_state: Arc<Mutex<ExternalMigrationState>>,
    pub critical_operation_guard: CriticalOperationGuard,
}

impl AppState {
    pub fn new(
        app_data_dir: &std::path::Path,
        fixture_dir: PathBuf,
    ) -> Result<Self, crate::error::AppError> {
        let postgres_manager = Arc::new(Mutex::new(PostgresManager::new(app_data_dir)));
        let settings = Arc::new(Mutex::new(SettingsStore::new(app_data_dir)?));
        let credentials = Arc::new(CredentialStore::new(app_data_dir)?);
        let database_service = DatabaseService::new(
            postgres_manager.clone(),
            settings.clone(),
            credentials.clone(),
        );

        Ok(Self {
            app_data_dir: app_data_dir.to_path_buf(),
            postgres_manager,
            settings,
            database_service,
            fixture_dir,
            scan_state: Arc::new(Mutex::new(ScanState::default())),
            commit_state: Arc::new(Mutex::new(CommitState::default())),
            external_migration_state: Arc::new(Mutex::new(ExternalMigrationState::default())),
            critical_operation_guard: CriticalOperationGuard::default(),
        })
    }
}

#[cfg(test)]
mod critical_operation_guard_tests {
    use super::*;

    #[test]
    fn active_commit_blocks_lifecycle_without_losing_task_registration() {
        let guard = CriticalOperationGuard::default();
        let commit = guard.begin_task(CriticalTaskKind::Commit).unwrap();

        let error = guard
            .begin_operation(CriticalOperationKind::ShutdownDatabase)
            .expect_err("active commit must block shutdown");
        assert_eq!(error, "cannot stop database while import commit is running");
        assert_eq!(guard.snapshot().active_task_kinds, vec!["commit"]);

        drop(commit);
        let shutdown = guard
            .begin_operation(CriticalOperationKind::ShutdownDatabase)
            .expect("shutdown may start after commit completion");
        assert_eq!(
            guard.snapshot().active_operation.as_deref(),
            Some("shutdown_database")
        );
        drop(shutdown);
        assert!(!guard.snapshot().is_blocked);
    }

    #[test]
    fn lifecycle_operation_closes_task_start_toctou_window() {
        let guard = CriticalOperationGuard::default();
        let operation = guard
            .begin_operation(CriticalOperationKind::SwitchToManagedDatabase)
            .unwrap();

        let error = guard
            .begin_task(CriticalTaskKind::Scan)
            .expect_err("scan must not start inside the lifecycle boundary");
        assert_eq!(
            error,
            "cannot start import scan while database switch is running"
        );

        drop(operation);
        assert!(guard.begin_task(CriticalTaskKind::Scan).is_ok());
    }

    #[test]
    fn external_migration_is_exclusive_with_database_tasks() {
        let guard = CriticalOperationGuard::default();
        let scan = guard.begin_task(CriticalTaskKind::Scan).unwrap();
        let error = guard
            .begin_task(CriticalTaskKind::ExternalMigration)
            .expect_err("migration must not start during scan");
        assert_eq!(
            error,
            "cannot start external database migration while import scan is running"
        );
        drop(scan);

        let migration = guard
            .begin_task(CriticalTaskKind::ExternalMigration)
            .unwrap();
        let error = guard
            .begin_task(CriticalTaskKind::Recovery)
            .expect_err("recovery must not start during migration");
        assert_eq!(
            error,
            "cannot start transaction recovery while external database migration is running"
        );
        drop(migration);
    }

    #[test]
    fn status_reports_all_active_task_kinds_in_stable_order() {
        let guard = CriticalOperationGuard::default();
        let scan = guard.begin_task(CriticalTaskKind::Scan).unwrap();
        let commit = guard.begin_task(CriticalTaskKind::Commit).unwrap();

        let status = guard.snapshot();
        assert!(status.is_blocked);
        assert_eq!(status.active_task_kinds, vec!["scan", "commit"]);
        assert!(status
            .blocking_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("import scan, import commit")));

        drop(scan);
        drop(commit);
        assert!(!guard.snapshot().is_blocked);
    }
}
