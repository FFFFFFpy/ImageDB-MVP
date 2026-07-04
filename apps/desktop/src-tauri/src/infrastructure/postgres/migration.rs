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
const MIGRATION_0009: &str =
    include_str!("../../../migrations/0009_drop_redundant_snapshot_hash.sql");

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_initial", MIGRATION_0001),
    ("0002_indexes", MIGRATION_0002),
    ("0003_commit_indexes", MIGRATION_0003),
    ("0004_match_indexes", MIGRATION_0004),
    ("0005_import_plans", MIGRATION_0005),
    ("0006_idempotency", MIGRATION_0006),
    ("0007_transaction_links", MIGRATION_0007),
    ("0008_source_album_snapshots", MIGRATION_0008),
    ("0009_drop_redundant_snapshot_hash", MIGRATION_0009),
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

    pub fn known_versions() -> Vec<&'static str> {
        MIGRATIONS.iter().map(|(version, _)| *version).collect()
    }

    pub fn latest_version() -> &'static str {
        MIGRATIONS
            .last()
            .map(|(version, _)| *version)
            .expect("at least one database migration must be embedded")
    }

    pub fn validate_applied_versions(applied: &[String]) -> Result<(), String> {
        let known = Self::known_versions();

        for version in applied {
            if !known.contains(&version.as_str()) {
                return Err(format!(
                    "unknown ImageDB migration version '{version}'; refusing to use a newer or incompatible database"
                ));
            }
        }

        for (idx, version) in applied.iter().enumerate() {
            if version != known[idx] {
                return Err(format!(
                    "non-contiguous ImageDB migration history: expected '{}' at position {}, found '{}'",
                    known[idx],
                    idx + 1,
                    version
                ));
            }
        }

        Ok(())
    }

    pub async fn run_pending(client: &mut Client) -> Result<Vec<String>, AppError> {
        Self::ensure_schema_migrations_table(client).await?;

        let applied = Self::get_applied_migrations(client).await?;
        Self::validate_applied_versions(&applied).map_err(|e| {
            AppError::Internal(format!("incompatible ImageDB migration history: {e}"))
        })?;
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
        assert_eq!(MIGRATIONS.len(), 9);
        assert!(MIGRATION_0001.contains("CREATE TABLE app_meta"));
        assert!(MIGRATION_0002.contains("CREATE INDEX"));
        assert!(MIGRATION_0003.contains("idx_library_albums_root_path"));
        assert!(MIGRATION_0004.contains("perceptual_band_0"));
        assert!(MIGRATION_0005.contains("import_plans"));
        assert!(MIGRATION_0006.contains("chk_import_run_state"));
        assert!(MIGRATION_0007.contains("plan_hash"));
        assert!(MIGRATION_0008.contains("source_album_snapshots"));
        assert!(MIGRATION_0009.contains("source_snapshot_hash"));
    }

    #[test]
    fn test_migration_versions_ordered() {
        assert_eq!(
            MigrationRunner::known_versions(),
            vec![
                "0001_initial",
                "0002_indexes",
                "0003_commit_indexes",
                "0004_match_indexes",
                "0005_import_plans",
                "0006_idempotency",
                "0007_transaction_links",
                "0008_source_album_snapshots",
                "0009_drop_redundant_snapshot_hash"
            ]
        );
        assert_eq!(
            MigrationRunner::latest_version(),
            "0009_drop_redundant_snapshot_hash"
        );
    }

    #[test]
    fn test_validate_applied_versions_accepts_empty_and_prefix() {
        assert!(MigrationRunner::validate_applied_versions(&[]).is_ok());
        assert!(MigrationRunner::validate_applied_versions(&[
            "0001_initial".to_string(),
            "0002_indexes".to_string(),
        ])
        .is_ok());
    }

    #[test]
    fn test_validate_applied_versions_rejects_unknown_or_non_contiguous_history() {
        let unknown = MigrationRunner::validate_applied_versions(&[
            "0001_initial".to_string(),
            "9999_future".to_string(),
        ]);
        assert!(unknown
            .expect_err("unknown future migration should fail")
            .contains("unknown ImageDB migration version"));

        let gap = MigrationRunner::validate_applied_versions(&[
            "0001_initial".to_string(),
            "0003_commit_indexes".to_string(),
        ]);
        assert!(gap
            .expect_err("non-contiguous migration history should fail")
            .contains("non-contiguous ImageDB migration history"));
    }
}
