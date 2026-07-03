use crate::error::AppError;
use tokio_postgres::Client;

const MIGRATION_0001: &str = include_str!("../../../migrations/0001_initial.sql");
const MIGRATION_0002: &str = include_str!("../../../migrations/0002_indexes.sql");
const MIGRATION_0003: &str = include_str!("../../../migrations/0003_commit_indexes.sql");
const MIGRATION_0004: &str = include_str!("../../../migrations/0004_match_indexes.sql");
const MIGRATION_0005: &str = include_str!("../../../migrations/0005_import_plans.sql");
const MIGRATION_0006: &str = include_str!("../../../migrations/0006_idempotency.sql");
const MIGRATION_0007: &str = include_str!("../../../migrations/0007_transaction_links.sql");
const MIGRATION_0008: &str = include_str!("../../../migrations/0008_source_album_snapshots.sql");

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_initial", MIGRATION_0001),
    ("0002_indexes", MIGRATION_0002),
    ("0003_commit_indexes", MIGRATION_0003),
    ("0004_match_indexes", MIGRATION_0004),
    ("0005_import_plans", MIGRATION_0005),
    ("0006_idempotency", MIGRATION_0006),
    ("0007_transaction_links", MIGRATION_0007),
    ("0008_source_album_snapshots", MIGRATION_0008),
];

pub struct MigrationRunner;

impl MigrationRunner {
    async fn ensure_schema_migrations_table(client: &Client) -> Result<(), AppError> {
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    version TEXT PRIMARY KEY,
                    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
                )",
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to create schema_migrations table: {e}"))
            })?;
        Ok(())
    }

    pub async fn get_applied_migrations(client: &Client) -> Result<Vec<String>, AppError> {
        Self::ensure_schema_migrations_table(client).await?;

        let rows = client
            .query(
                "SELECT version FROM schema_migrations ORDER BY version",
                &[],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query schema_migrations: {e}")))?;

        Ok(rows.iter().map(|r| r.get::<_, String>("version")).collect())
    }

    pub async fn run_pending(client: &mut Client) -> Result<Vec<String>, AppError> {
        Self::ensure_schema_migrations_table(client).await?;

        let applied = Self::get_applied_migrations(client).await?;
        let mut newly_applied = Vec::new();

        for (version, sql) in MIGRATIONS {
            if applied.contains(&version.to_string()) {
                tracing::info!("Migration {version} already applied, skipping");
                continue;
            }

            tracing::info!("Applying migration {version}");

            let transaction = client.transaction().await.map_err(|e| {
                AppError::Internal(format!("failed to begin transaction for {version}: {e}"))
            })?;

            transaction
                .batch_execute(sql)
                .await
                .map_err(|e| AppError::Internal(format!("migration {version} failed: {e}")))?;

            transaction
                .execute(
                    "INSERT INTO schema_migrations (version) VALUES ($1)",
                    &[version],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to record migration {version}: {e}"))
                })?;

            transaction.commit().await.map_err(|e| {
                AppError::Internal(format!("failed to commit migration {version}: {e}"))
            })?;

            newly_applied.push(version.to_string());
            tracing::info!("Migration {version} applied successfully");
        }

        Ok(newly_applied)
    }

    pub async fn current_version(client: &Client) -> Result<Option<String>, AppError> {
        let applied = Self::get_applied_migrations(client).await?;
        Ok(applied.into_iter().last())
    }

    pub fn total_migrations() -> usize {
        MIGRATIONS.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_embedded() {
        assert_eq!(MIGRATIONS.len(), 8);
        assert!(MIGRATION_0001.contains("CREATE TABLE app_meta"));
        assert!(MIGRATION_0002.contains("CREATE INDEX"));
        assert!(MIGRATION_0003.contains("idx_library_albums_root_path"));
        assert!(MIGRATION_0004.contains("perceptual_band_0"));
        assert!(MIGRATION_0005.contains("import_plans"));
        assert!(MIGRATION_0006.contains("chk_import_run_state"));
        assert!(MIGRATION_0007.contains("plan_hash"));
        assert!(MIGRATION_0008.contains("source_album_snapshots"));
    }

    #[test]
    fn test_migration_versions_ordered() {
        let versions: Vec<&str> = MIGRATIONS.iter().map(|(v, _)| *v).collect();
        assert_eq!(
            versions,
            vec![
                "0001_initial",
                "0002_indexes",
                "0003_commit_indexes",
                "0004_match_indexes",
                "0005_import_plans",
                "0006_idempotency",
                "0007_transaction_links",
                "0008_source_album_snapshots"
            ]
        );
    }
}
