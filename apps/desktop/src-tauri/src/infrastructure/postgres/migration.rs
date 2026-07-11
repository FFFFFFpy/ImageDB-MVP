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
const MIGRATION_0010: &str = include_str!("../../../migrations/0010_library_root_leases.sql");
const MIGRATION_0011: &str = include_str!("../../../migrations/0011_album_workflow_state.sql");
const MIGRATION_0012: &str = include_str!("../../../migrations/0012_album_workflow_repair.sql");

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
    ("0010_library_root_leases", MIGRATION_0010),
    ("0011_album_workflow_state", MIGRATION_0011),
    ("0012_album_workflow_repair", MIGRATION_0012),
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

            transaction.batch_execute(sql).await.map_err(|e| {
                let detail = e
                    .as_db_error()
                    .map(|db_error| db_error.message().to_string())
                    .unwrap_or_else(|| e.to_string());
                AppError::Internal(format!("migration {version} failed: {detail}"))
            })?;

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
        assert_eq!(MIGRATIONS.len(), 12);
        assert!(MIGRATION_0001.contains("CREATE TABLE app_meta"));
        assert!(MIGRATION_0002.contains("CREATE INDEX"));
        assert!(MIGRATION_0003.contains("idx_library_albums_root_path"));
        assert!(MIGRATION_0004.contains("perceptual_band_0"));
        assert!(MIGRATION_0005.contains("import_plans"));
        assert!(MIGRATION_0006.contains("chk_import_run_state"));
        assert!(MIGRATION_0007.contains("plan_hash"));
        assert!(MIGRATION_0008.contains("source_album_snapshots"));
        assert!(MIGRATION_0009.contains("source_snapshot_hash"));
        assert!(MIGRATION_0010.contains("library_root_leases"));
        assert!(MIGRATION_0011.contains("analysis_started_at"));
        assert!(MIGRATION_0012.contains("DELETE FROM duplicate_candidates"));
        assert!(MIGRATION_0012.contains("fingerprinted_count"));
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
                "0009_drop_redundant_snapshot_hash",
                "0010_library_root_leases",
                "0011_album_workflow_state",
                "0012_album_workflow_repair"
            ]
        );
        assert_eq!(
            MigrationRunner::latest_version(),
            "0012_album_workflow_repair"
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

    #[cfg(feature = "real-db-tests")]
    async fn apply_migration_prefix(client: &mut Client, count: usize) {
        MigrationRunner::ensure_schema_migrations_table(client)
            .await
            .unwrap();
        for (version, sql) in MIGRATIONS.iter().take(count) {
            let transaction = client.transaction().await.unwrap();
            transaction.batch_execute(sql).await.unwrap();
            transaction
                .execute(
                    "INSERT INTO schema_migrations (version) VALUES ($1)",
                    &[version],
                )
                .await
                .unwrap();
            transaction.commit().await.unwrap();
        }
    }

    #[cfg(feature = "real-db-tests")]
    async fn apply_migrations_through_0011(client: &mut Client) {
        apply_migration_prefix(client, 11).await;
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_migration_0012_active_connection_upgrades_0010_dashboard_schema() {
        use crate::repositories::import_repository::ImportRepository;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut manager = crate::infrastructure::postgres::PostgresManager::new(tmp.path());
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);

        let (mut old_client, old_handle) = manager.connect_raw().await.unwrap();
        apply_migration_prefix(&mut old_client, 10).await;
        let library_root_id = ImportRepository::upsert_default_library_root(&old_client)
            .await
            .unwrap();
        let import_run_id = ImportRepository::create_import_run(
            &old_client,
            "C:/active-upgrade/source",
            library_root_id,
        )
        .await
        .unwrap();
        let old_album_id = uuid::Uuid::new_v4();
        old_client
            .execute(
                "INSERT INTO import_albums
                    (id, import_run_id, source_path, source_name, state)
                 VALUES ($1, $2, $3, $4, 'pending')",
                &[
                    &old_album_id,
                    &import_run_id,
                    &"C:/active-upgrade/source/album-a",
                    &"album-a",
                ],
            )
            .await
            .unwrap();
        drop(old_client);
        old_handle.abort();

        // This is the production connection boundary used by dashboard and
        // scan commands after an application upgrade. It must upgrade an
        // already configured 0010 database before returning the client.
        let (client, handle) = manager.connect().await.unwrap();
        assert_eq!(
            MigrationRunner::current_version(&client)
                .await
                .unwrap()
                .as_deref(),
            Some(MigrationRunner::latest_version())
        );
        let runs = ImportRepository::list_import_runs_summary(&client)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].import_run_id, import_run_id.to_string());
        assert_eq!(runs[0].total_albums, 1);
        assert_eq!(runs[0].pending_albums, 1);

        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_migration_0012_repairs_original_and_edited_0011_rows() {
        use crate::domain::import_state::{
            Decision, DecisionSource, DecodeState, DuplicateScope, ImportImageState, MatchType,
        };
        use crate::repositories::import_repository::{
            ImportRepository, NewDuplicateCandidate, NewImportImage,
        };
        use tempfile::TempDir;
        use uuid::Uuid;

        let tmp = TempDir::new().unwrap();
        let mut manager = crate::infrastructure::postgres::PostgresManager::new(tmp.path());
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);
        let (mut client, handle) = manager.connect_raw().await.unwrap();
        apply_migrations_through_0011(&mut client).await;

        let library_root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        let import_run_id = ImportRepository::create_import_run(
            &client,
            "C:/migration-0012/source",
            library_root_id,
        )
        .await
        .unwrap();

        async fn insert_image(client: &Client, album_id: Uuid, marker: u8) -> Uuid {
            ImportRepository::insert_import_image(
                client,
                NewImportImage {
                    album_id,
                    source_path: format!("C:/migration-0012/{marker}.png"),
                    relative_path: format!("album/{marker}.png"),
                    file_size: 10,
                    modified_at: None,
                    width: Some(1),
                    height: Some(1),
                    format: Some("png".to_string()),
                    decode_state: DecodeState::Decoded,
                    blake3: Some(vec![marker; 32]),
                    pixel_hash: Some(vec![marker; 8]),
                    gradient_hash: Some(vec![marker; 8]),
                    block_hash: Some(vec![marker; 8]),
                    median_hash: Some(vec![marker; 8]),
                    fingerprint_version: Some("test".to_string()),
                    state: ImportImageState::Fingerprinted,
                },
            )
            .await
            .unwrap()
        }

        let original_stale_album = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            "C:/migration-0012/source/original-stale",
            "original-stale",
        )
        .await
        .unwrap();
        let edited_pending_album = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            "C:/migration-0012/source/edited-pending",
            "edited-pending",
        )
        .await
        .unwrap();
        let terminal_album = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            "C:/migration-0012/source/terminal",
            "terminal",
        )
        .await
        .unwrap();

        let stale_image = insert_image(&client, original_stale_album, 1).await;
        let _pending_image = insert_image(&client, edited_pending_album, 2).await;
        let terminal_image = insert_image(&client, terminal_album, 3).await;
        ImportRepository::insert_duplicate_candidate(
            &client,
            NewDuplicateCandidate {
                import_run_id,
                source_image_id: stale_image,
                candidate_source_image_id: Some(terminal_image),
                candidate_library_image_id: None,
                scope: DuplicateScope::CrossAlbum,
                match_type: MatchType::FileExact,
                blake3_equal: true,
                pixel_hash_equal: false,
                gradient_distance: None,
                block_distance: None,
                median_distance: None,
                transform_type: None,
                confidence: Some(1.0),
                decision: Some(Decision::AutoDuplicate),
                decision_source: Some(DecisionSource::ExactRule),
            },
        )
        .await
        .unwrap();
        client
            .execute(
                "UPDATE import_albums SET state = 'scanning' WHERE id = $1",
                &[&original_stale_album],
            )
            .await
            .unwrap();
        // Simulate a database that already ran the briefly edited 0011: the
        // state is pending, but partial import rows were not cleaned.
        client
            .execute(
                "UPDATE import_albums SET state = 'pending' WHERE id = $1",
                &[&edited_pending_album],
            )
            .await
            .unwrap();
        client
            .execute(
                "UPDATE import_albums SET state = 'completed' WHERE id = $1",
                &[&terminal_album],
            )
            .await
            .unwrap();

        let applied = MigrationRunner::run_pending(&mut client).await.unwrap();
        assert_eq!(applied, vec!["0012_album_workflow_repair"]);

        for album_id in [original_stale_album, edited_pending_album] {
            let row = client
                .query_one(
                    "SELECT state, image_count, fingerprinted_count
                     FROM import_albums WHERE id = $1",
                    &[&album_id],
                )
                .await
                .unwrap();
            assert_eq!(row.get::<_, String>("state"), "pending");
            assert_eq!(row.get::<_, i32>("image_count"), 0);
            assert_eq!(row.get::<_, i32>("fingerprinted_count"), 0);
            let image_count: i64 = client
                .query_one(
                    "SELECT COUNT(*) FROM import_images WHERE import_album_id = $1",
                    &[&album_id],
                )
                .await
                .unwrap()
                .get(0);
            assert_eq!(image_count, 0);
        }
        let terminal = client
            .query_one(
                "SELECT state, image_count, fingerprinted_count,
                        duplicate_candidate_count, review_candidate_count
                 FROM import_albums WHERE id = $1",
                &[&terminal_album],
            )
            .await
            .unwrap();
        assert_eq!(terminal.get::<_, String>("state"), "analyzed");
        assert_eq!(terminal.get::<_, i32>("image_count"), 1);
        assert_eq!(terminal.get::<_, i32>("fingerprinted_count"), 1);
        assert_eq!(terminal.get::<_, i32>("duplicate_candidate_count"), 0);
        assert_eq!(terminal.get::<_, i32>("review_candidate_count"), 0);
        let candidate_count: i64 = client
            .query_one("SELECT COUNT(*) FROM duplicate_candidates", &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(candidate_count, 0);
        assert_eq!(
            MigrationRunner::current_version(&client).await.unwrap(),
            Some("0012_album_workflow_repair".to_string())
        );

        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_migration_0012_rejects_stale_plan_evidence_atomically() {
        use crate::repositories::import_repository::ImportRepository;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut manager = crate::infrastructure::postgres::PostgresManager::new(tmp.path());
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);
        let (mut client, handle) = manager.connect_raw().await.unwrap();
        apply_migrations_through_0011(&mut client).await;

        let library_root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        let import_run_id = ImportRepository::create_import_run(
            &client,
            "C:/migration-0012/guarded",
            library_root_id,
        )
        .await
        .unwrap();
        let album_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            "C:/migration-0012/guarded/album",
            "album",
        )
        .await
        .unwrap();
        let plan_id = ImportRepository::create_import_plan(
            &client,
            import_run_id,
            1,
            "test",
            library_root_id,
        )
        .await
        .unwrap();
        ImportRepository::insert_plan_album(&client, plan_id, album_id, "album", 0)
            .await
            .unwrap();

        let error = MigrationRunner::run_pending(&mut client)
            .await
            .expect_err("stale rows with plan evidence must block migration atomically");
        assert!(error.to_string().contains("referenced by an import plan"));
        assert_eq!(
            MigrationRunner::current_version(&client).await.unwrap(),
            Some("0011_album_workflow_state".to_string())
        );
        let album_state: String = client
            .query_one(
                "SELECT state FROM import_albums WHERE id = $1",
                &[&album_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(album_state, "pending");
        let plan_album_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_plan_albums WHERE import_album_id = $1",
                &[&album_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(plan_album_count, 1);

        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
    }
}
