use crate::domain::{
    ConnectionConfig, DatabaseMode, DatabaseState, DatabaseStatus, ExternalCheckResult,
    ManagedDbConfig,
};
use crate::error::AppError;
use crate::infrastructure::postgres::migration::MigrationRunner;
use crate::infrastructure::postgres::PostgresManager;
use crate::infrastructure::settings::SettingsStore;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct DatabaseService {
    postgres_manager: Arc<Mutex<PostgresManager>>,
    settings: Arc<Mutex<SettingsStore>>,
}

impl DatabaseService {
    pub fn new(
        postgres_manager: Arc<Mutex<PostgresManager>>,
        settings: Arc<Mutex<SettingsStore>>,
    ) -> Self {
        Self {
            postgres_manager,
            settings,
        }
    }

    pub async fn get_state(&self) -> DatabaseState {
        let mgr = self.postgres_manager.lock().await;
        let settings = self.settings.lock().await;
        let mode = settings
            .get()
            .database_mode
            .as_deref()
            .and_then(DatabaseMode::from_str_opt);

        let (status, pgvector, migration_version, diagnostics) =
            if mgr.is_server_running() && mgr.binaries_available() {
                match mgr.connect().await {
                    Ok((client, handle)) => {
                        let pgvector = client
                            .query_one(
                                "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='vector')",
                                &[],
                            )
                            .await
                            .map(|row| row.get::<_, bool>(0))
                            .unwrap_or(false);

                        let migration_version = MigrationRunner::current_version(&client)
                            .await
                            .ok()
                            .flatten();

                        handle.abort();
                        (
                            DatabaseStatus::Connected,
                            pgvector,
                            migration_version,
                            vec![],
                        )
                    }
                    Err(e) => (
                        DatabaseStatus::Error(e.to_string()),
                        false,
                        None,
                        vec![e.to_string()],
                    ),
                }
            } else if !mgr.binaries_available() {
                (
                    DatabaseStatus::BinariesMissing(
                        "PostgreSQL binaries not found on this system".to_string(),
                    ),
                    false,
                    None,
                    mgr.diagnostics().to_vec(),
                )
            } else {
                (DatabaseStatus::NotInitialized, false, None, vec![])
            };

        let managed_config = if mgr.binaries_available() {
            Some(ManagedDbConfig {
                data_dir: mgr.data_dir().display().to_string(),
                port: mgr.port(),
                username: mgr.username().to_string(),
                database: mgr.database().to_string(),
            })
        } else {
            None
        };

        let external_config = {
            let s = settings.get();
            if s.external_host.is_some() {
                Some(ConnectionConfig {
                    host: s.external_host.clone().unwrap_or_default(),
                    port: s.external_port.unwrap_or(5432),
                    database: s
                        .external_database
                        .clone()
                        .unwrap_or_else(|| "imagedb".to_string()),
                    username: s.external_username.clone().unwrap_or_default(),
                    password: None,
                })
            } else {
                None
            }
        };

        DatabaseState {
            mode,
            status,
            managed_config,
            external_config,
            pgvector_available: pgvector,
            migration_version,
            diagnostics,
        }
    }

    pub async fn initialize_managed(&self) -> Result<DatabaseState, AppError> {
        let mut mgr = self.postgres_manager.lock().await;

        if !mgr.binaries_available() {
            return Ok(DatabaseState {
                mode: Some(DatabaseMode::ManagedLocal),
                status: DatabaseStatus::BinariesMissing(
                    "PostgreSQL binaries not found. Install PostgreSQL or place binaries alongside the application.".to_string(),
                ),
                managed_config: None,
                external_config: None,
                pgvector_available: false,
                migration_version: None,
                diagnostics: mgr.diagnostics().to_vec(),
            });
        }

        let probe_result = mgr.initialize().await?;

        if !probe_result.connection_ok {
            return Ok(DatabaseState {
                mode: Some(DatabaseMode::ManagedLocal),
                status: DatabaseStatus::Error(
                    "Database initialization failed - connection test failed".to_string(),
                ),
                managed_config: Some(ManagedDbConfig {
                    data_dir: probe_result.data_dir.unwrap_or_default(),
                    port: probe_result.port.unwrap_or(0),
                    username: mgr.username().to_string(),
                    database: mgr.database().to_string(),
                }),
                external_config: None,
                pgvector_available: false,
                migration_version: None,
                diagnostics: probe_result.diagnostics,
            });
        }

        let (mut client, handle) = mgr.connect().await?;
        let applied = MigrationRunner::run_pending(&mut client).await?;
        let version = MigrationRunner::current_version(&client).await?;
        handle.abort();

        let mut settings = self.settings.lock().await;
        settings.set_database_mode("managed_local")?;
        settings.set_first_run_completed(true)?;

        Ok(DatabaseState {
            mode: Some(DatabaseMode::ManagedLocal),
            status: DatabaseStatus::Connected,
            managed_config: Some(ManagedDbConfig {
                data_dir: probe_result.data_dir.unwrap_or_default(),
                port: probe_result.port.unwrap_or(0),
                username: mgr.username().to_string(),
                database: mgr.database().to_string(),
            }),
            external_config: None,
            pgvector_available: probe_result.pgvector_available,
            migration_version: version,
            diagnostics: {
                let mut d = probe_result.diagnostics;
                if !applied.is_empty() {
                    d.push(format!("Applied migrations: {}", applied.join(", ")));
                }
                d
            },
        })
    }

    pub async fn test_external_connection(
        &self,
        config: &ConnectionConfig,
    ) -> Result<ExternalCheckResult, AppError> {
        let conn_str = config.connection_string();
        let mut diagnostics = Vec::new();

        let (client, handle) = match tokio_postgres::connect(&conn_str, tokio_postgres::NoTls).await
        {
            Ok((client, conn)) => {
                let handle = tokio::spawn(async move {
                    if let Err(e) = conn.await {
                        tracing::warn!("External connection lost: {e}");
                    }
                });
                diagnostics.push("Connection successful".to_string());
                (client, handle)
            }
            Err(e) => {
                diagnostics.push(format!("Connection failed: {e}"));
                return Ok(ExternalCheckResult {
                    connection_ok: false,
                    version: None,
                    version_ok: false,
                    pgvector_available: false,
                    can_create_tables: false,
                    diagnostics,
                });
            }
        };

        let version = client
            .query_one("SELECT version()", &[])
            .await
            .ok()
            .and_then(|row| row.try_get::<_, String>(0).ok());

        let (version_ok, version_diagnostic) = match version.as_deref() {
            Some(v) => match parse_postgres_major(v) {
                Some(n) if n >= 14 => (true, format!("PostgreSQL version: {v} (major {n})")),
                Some(n) => (
                    false,
                    format!(
                        "PostgreSQL version {v} is below the minimum supported major version 14 \
                         (detected major {n})"
                    ),
                ),
                None => (
                    false,
                    format!(
                        "Could not parse PostgreSQL major version from: {v} \
                         (expected a substring like 'PostgreSQL <number>')"
                    ),
                ),
            },
            None => (
                false,
                "SELECT version() returned no usable string".to_string(),
            ),
        };

        diagnostics.push(version_diagnostic);

        let pgvector_available = client
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_available_extensions WHERE name='vector')",
                &[],
            )
            .await
            .map(|row| row.get::<_, bool>(0))
            .unwrap_or(false);

        if pgvector_available {
            diagnostics.push("pgvector extension available".to_string());
        } else {
            diagnostics.push("Warning: pgvector extension not available".to_string());
        }

        let can_create_tables = match client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS _imagedb_permission_test (id int);
                 DROP TABLE IF EXISTS _imagedb_permission_test;",
            )
            .await
        {
            Ok(()) => {
                diagnostics.push("Table creation permission: OK".to_string());
                true
            }
            Err(e) => {
                diagnostics.push(format!("Table creation permission: FAILED - {e}"));
                false
            }
        };

        handle.abort();

        Ok(ExternalCheckResult {
            connection_ok: true,
            version,
            version_ok,
            pgvector_available,
            can_create_tables,
            diagnostics,
        })
    }

    pub async fn initialize_external(
        &self,
        config: &ConnectionConfig,
    ) -> Result<DatabaseState, AppError> {
        let check = self.test_external_connection(config).await?;

        if !check.connection_ok {
            return Ok(DatabaseState {
                mode: Some(DatabaseMode::External),
                status: DatabaseStatus::Error("External connection failed".to_string()),
                managed_config: None,
                external_config: Some(config.clone()),
                pgvector_available: false,
                migration_version: None,
                diagnostics: check.diagnostics,
            });
        }

        let conn_str = config.connection_string();
        let (mut client, conn) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
            .await
            .map_err(|e| {
                AppError::PostgresUnavailable(format!("external connection failed: {e}"))
            })?;
        let handle = tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("External connection lost: {e}");
            }
        });

        let applied = MigrationRunner::run_pending(&mut client).await?;
        let version = MigrationRunner::current_version(&client).await?;
        handle.abort();

        let mut settings = self.settings.lock().await;
        let current = settings.get().clone();
        settings.update(crate::infrastructure::settings::AppSettings {
            database_mode: Some("external".to_string()),
            external_host: Some(config.host.clone()),
            external_port: Some(config.port),
            external_database: Some(config.database.clone()),
            external_username: Some(config.username.clone()),
            first_run_completed: true,
            ..current
        })?;

        Ok(DatabaseState {
            mode: Some(DatabaseMode::External),
            status: DatabaseStatus::Connected,
            managed_config: None,
            external_config: Some(config.clone()),
            pgvector_available: check.pgvector_available,
            migration_version: version,
            diagnostics: {
                let mut d = check.diagnostics;
                if !applied.is_empty() {
                    d.push(format!("Applied migrations: {}", applied.join(", ")));
                }
                d
            },
        })
    }
}

fn parse_postgres_major(version: &str) -> Option<u32> {
    let marker = "PostgreSQL ";
    let idx = version.find(marker)?;
    let after = &version[idx + marker.len()..];
    let digits_end = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    if digits_end == 0 {
        return None;
    }
    after[..digits_end].parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_postgres_major_standard() {
        let v = "PostgreSQL 16.4 on x86_64-pc-linux-gnu, compiled by gcc 11.4.0, 64-bit";
        assert_eq!(parse_postgres_major(v), Some(16));
    }

    #[test]
    fn parse_postgres_major_future() {
        let v = "PostgreSQL 18.0 on x86_64-pc-linux-gnu";
        assert_eq!(parse_postgres_major(v), Some(18));
    }

    #[test]
    fn parse_postgres_major_dev() {
        let v = "PostgreSQL 19devel on x86_64";
        assert_eq!(parse_postgres_major(v), Some(19));
    }

    #[test]
    fn parse_postgres_major_old() {
        let v = "PostgreSQL 13.14 on x86_64";
        assert_eq!(parse_postgres_major(v), Some(13));
    }

    #[test]
    fn parse_postgres_major_malformed() {
        assert_eq!(parse_postgres_major("MySQL 8.0"), None);
        assert_eq!(parse_postgres_major("PostgreSQL abc"), None);
        assert_eq!(parse_postgres_major(""), None);
    }
}
