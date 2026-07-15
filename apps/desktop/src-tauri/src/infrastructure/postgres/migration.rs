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
const MIGRATION_0013: &str =
    include_str!("../../../migrations/0013_workflow_escape_and_candidate_uniqueness.sql");
const MIGRATION_0014: &str =
    include_str!("../../../migrations/0014_candidate_review_semantics_and_abandoned_filters.sql");
const MIGRATION_0015: &str = include_str!("../../../migrations/0015_fingerprint_v2.sql");

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
    (
        "0013_workflow_escape_and_candidate_uniqueness",
        MIGRATION_0013,
    ),
    (
        "0014_candidate_review_semantics_and_abandoned_filters",
        MIGRATION_0014,
    ),
    ("0015_fingerprint_v2", MIGRATION_0015),
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
        assert_eq!(MIGRATIONS.len(), 15);
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
        assert!(MIGRATION_0013.contains("'abandoned'"));
        assert!(MIGRATION_0013.contains("idx_duplicate_candidates_library_pair"));
        assert!(MIGRATION_0013.contains("conflicting normalized review outcomes"));
        assert!(MIGRATION_0014.contains("enforce_review_decision_semantics"));
        assert!(MIGRATION_0015.contains("block_hash_16"));
        assert!(MIGRATION_0015.contains("double_gradient_distance_ratio"));
        assert!(MIGRATION_0015.contains("double_gradient_hash_32 IS NOT NULL"));
        assert!(MIGRATION_0015.contains("DROP INDEX IF EXISTS idx_import_images_blake3"));
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
                "0012_album_workflow_repair",
                "0013_workflow_escape_and_candidate_uniqueness",
                "0014_candidate_review_semantics_and_abandoned_filters",
                "0015_fingerprint_v2"
            ]
        );
        assert_eq!(MigrationRunner::latest_version(), "0015_fingerprint_v2");
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

    #[cfg(feature = "real-db-tests")]
    async fn insert_duplicate_pair_fixture(
        client: &Client,
        library_pair: bool,
    ) -> (uuid::Uuid, uuid::Uuid, uuid::Uuid, uuid::Uuid, uuid::Uuid) {
        use uuid::Uuid;

        let root_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let album_id = Uuid::new_v4();
        let image_a = Uuid::new_v4();
        let image_b = Uuid::new_v4();
        let candidate_1 = Uuid::new_v4();
        let candidate_2 = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO library_roots (id, path, display_name) VALUES ($1, $2, $3)",
                &[
                    &root_id,
                    &format!("C:/migration-root-{root_id}"),
                    &"fixture",
                ],
            )
            .await
            .unwrap();
        client
            .execute(
                "INSERT INTO import_runs (id, source_root, library_root_id, state, policy_version)
                 VALUES ($1, $2, $3, 'review_required', 'fixture')",
                &[&run_id, &format!("C:/migration-source-{run_id}"), &root_id],
            )
            .await
            .unwrap();
        client
            .execute(
                "INSERT INTO import_albums (id, import_run_id, source_path, source_name, state)
                 VALUES ($1, $2, $3, 'album', 'review_required')",
                &[
                    &album_id,
                    &run_id,
                    &format!("C:/migration-album-{album_id}"),
                ],
            )
            .await
            .unwrap();
        for (id, marker) in [(image_a, "a"), (image_b, "b")] {
            client
                .execute(
                    "INSERT INTO import_images
                        (id, import_album_id, source_path, relative_path, file_size, decode_state, state)
                     VALUES ($1, $2, $3, $4, 1, 'decoded', 'fingerprinted')",
                    &[
                        &id,
                        &album_id,
                        &format!("C:/migration/{marker}.png"),
                        &format!("{marker}.png"),
                    ],
                )
                .await
                .unwrap();
        }

        if library_pair {
            let library_album_id = Uuid::new_v4();
            client
                .execute(
                    "INSERT INTO library_albums
                        (id, library_root_id, display_name, relative_path, manifest_version,
                         manifest_hash, image_count, state)
                     VALUES ($1, $2, 'library', $3, '1', $4, 1, 'active')",
                    &[
                        &library_album_id,
                        &root_id,
                        &format!("library-{library_album_id}"),
                        &vec![1_u8],
                    ],
                )
                .await
                .unwrap();
            client
                .execute(
                    "INSERT INTO library_images
                        (id, album_id, relative_path, file_size, width, height, format, blake3,
                         fingerprint_version, state)
                     VALUES ($1, $2, 'library.png', 1, 1, 1, 'png', $3, '1', 'active')",
                    &[&image_b, &library_album_id, &vec![2_u8]],
                )
                .await
                .unwrap();
            for (id, match_type) in [(candidate_1, "pixel_exact"), (candidate_2, "file_exact")] {
                client.execute(
                    "INSERT INTO duplicate_candidates
                        (id, import_run_id, source_image_id, candidate_library_image_id, scope, match_type)
                     VALUES ($1, $2, $3, $4, 'library', $5)",
                    &[&id, &run_id, &image_a, &image_b, &match_type],
                ).await.unwrap();
            }
        } else {
            client.execute(
                "INSERT INTO duplicate_candidates
                    (id, import_run_id, source_image_id, candidate_source_image_id, scope, match_type)
                 VALUES ($1, $2, $3, $4, 'cross_album', 'pixel_exact'),
                        ($5, $2, $4, $3, 'cross_album', 'file_exact')",
                &[&candidate_1, &run_id, &image_a, &image_b, &candidate_2],
            ).await.unwrap();
        }
        (run_id, image_a, image_b, candidate_1, candidate_2)
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_migration_0013_preserves_equivalent_import_and_library_outcomes() {
        use tempfile::TempDir;
        use uuid::Uuid;

        let tmp = TempDir::new().unwrap();
        let mut manager = crate::infrastructure::postgres::PostgresManager::new(tmp.path());
        manager.initialize().await.unwrap();
        let (mut client, handle) = manager.connect_raw().await.unwrap();
        apply_migration_prefix(&mut client, 12).await;

        let (_, import_a, _, import_c1, import_c2) =
            insert_duplicate_pair_fixture(&client, false).await;
        client
            .execute(
                "INSERT INTO review_decisions (id, candidate_id, decision, selected_image_id)
             VALUES ($1, $2, 'keep_source', $3), ($4, $5, 'keep_candidate', $3)",
                &[
                    &Uuid::new_v4(),
                    &import_c1,
                    &import_a,
                    &Uuid::new_v4(),
                    &import_c2,
                ],
            )
            .await
            .unwrap();

        let (_, library_source, _, library_c1, library_c2) =
            insert_duplicate_pair_fixture(&client, true).await;
        client
            .execute(
                "INSERT INTO review_decisions (id, candidate_id, decision, selected_image_id)
             VALUES ($1, $2, 'keep_source', $3), ($4, $5, 'keep_source', $3)",
                &[
                    &Uuid::new_v4(),
                    &library_c1,
                    &library_source,
                    &Uuid::new_v4(),
                    &library_c2,
                ],
            )
            .await
            .unwrap();

        assert_eq!(
            MigrationRunner::run_pending(&mut client)
                .await
                .unwrap()
                .len(),
            3
        );
        for selected in [import_a, library_source] {
            let row = client
                .query_one(
                    "SELECT COUNT(*) AS candidates, COUNT(rd.id) AS decisions,
                        MIN(rd.selected_image_id::text) AS selected
                 FROM duplicate_candidates dc
                 LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                 WHERE dc.source_image_id = $1 OR dc.candidate_source_image_id = $1",
                    &[&selected],
                )
                .await
                .unwrap();
            assert_eq!(row.get::<_, i64>("candidates"), 1);
            assert_eq!(row.get::<_, i64>("decisions"), 1);
            assert_eq!(
                row.get::<_, Option<String>>("selected").as_deref(),
                Some(selected.to_string().as_str())
            );
        }
        let import_survivor = client.query_one(
            "SELECT dc.source_image_id, dc.candidate_source_image_id, rd.decision, rd.selected_image_id
             FROM duplicate_candidates dc JOIN review_decisions rd ON rd.candidate_id = dc.id
             WHERE dc.import_run_id = (SELECT import_run_id FROM duplicate_candidates WHERE source_image_id = $1 OR candidate_source_image_id = $1 LIMIT 1)",
            &[&import_a],
        ).await.unwrap();
        let source: Uuid = import_survivor.get("source_image_id");
        let candidate: Uuid = import_survivor.get("candidate_source_image_id");
        let decision: String = import_survivor.get("decision");
        let selected: Uuid = import_survivor.get("selected_image_id");
        assert_eq!(selected, import_a);
        assert_eq!(
            decision,
            if source == import_a {
                "keep_source"
            } else {
                "keep_candidate"
            }
        );
        assert!(source == import_a || candidate == import_a);

        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_migration_0013_rejects_conflicts_and_invalid_selected_atomically() {
        use tempfile::TempDir;
        use uuid::Uuid;

        for case in [
            "opposite_selection",
            "invalid_selected",
            "keep_all_conflict",
        ] {
            let tmp = TempDir::new().unwrap();
            let mut manager = crate::infrastructure::postgres::PostgresManager::new(tmp.path());
            manager.initialize().await.unwrap();
            let (mut client, handle) = manager.connect_raw().await.unwrap();
            apply_migration_prefix(&mut client, 12).await;
            let (_, image_a, image_b, c1, c2) = insert_duplicate_pair_fixture(&client, false).await;
            match case {
                "opposite_selection" => client.execute(
                    "INSERT INTO review_decisions (id, candidate_id, decision, selected_image_id)
                     VALUES ($1, $2, 'keep_source', $3), ($4, $5, 'keep_source', $6)",
                    &[&Uuid::new_v4(), &c1, &image_a, &Uuid::new_v4(), &c2, &image_b],
                ).await.unwrap(),
                "invalid_selected" => client.execute(
                    "INSERT INTO review_decisions (id, candidate_id, decision, selected_image_id)
                     VALUES ($1, $2, 'keep_source', $3)",
                    &[&Uuid::new_v4(), &c1, &image_b],
                ).await.unwrap(),
                _ => client.execute(
                    "INSERT INTO review_decisions (id, candidate_id, decision, selected_image_id)
                     VALUES ($1, $2, 'keep_all', NULL), ($3, $4, 'keep_candidate', $5)",
                    &[&Uuid::new_v4(), &c1, &Uuid::new_v4(), &c2, &image_a],
                ).await.unwrap(),
            };
            let before_candidates: i64 = client
                .query_one("SELECT COUNT(*) FROM duplicate_candidates", &[])
                .await
                .unwrap()
                .get(0);
            let before_decisions: i64 = client
                .query_one("SELECT COUNT(*) FROM review_decisions", &[])
                .await
                .unwrap()
                .get(0);
            let error = MigrationRunner::run_pending(&mut client)
                .await
                .expect_err(case);
            if case == "invalid_selected" {
                assert!(error
                    .to_string()
                    .contains("invalid review decision structure"));
            } else {
                assert!(error
                    .to_string()
                    .contains("conflicting normalized review outcomes"));
            }
            assert_eq!(
                MigrationRunner::current_version(&client)
                    .await
                    .unwrap()
                    .as_deref(),
                Some("0012_album_workflow_repair")
            );
            assert_eq!(
                client
                    .query_one("SELECT COUNT(*) FROM duplicate_candidates", &[])
                    .await
                    .unwrap()
                    .get::<_, i64>(0),
                before_candidates
            );
            assert_eq!(
                client
                    .query_one("SELECT COUNT(*) FROM review_decisions", &[])
                    .await
                    .unwrap()
                    .get::<_, i64>(0),
                before_decisions
            );
            drop(client);
            handle.abort();
            manager.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_migration_0013_rejects_library_outcome_conflict_atomically() {
        use tempfile::TempDir;
        use uuid::Uuid;
        let tmp = TempDir::new().unwrap();
        let mut manager = crate::infrastructure::postgres::PostgresManager::new(tmp.path());
        manager.initialize().await.unwrap();
        let (mut client, handle) = manager.connect_raw().await.unwrap();
        apply_migration_prefix(&mut client, 12).await;
        let (_, source, library, c1, c2) = insert_duplicate_pair_fixture(&client, true).await;
        client
            .execute(
                "INSERT INTO review_decisions (id, candidate_id, decision, selected_image_id)
             VALUES ($1, $2, 'keep_source', $3), ($4, $5, 'keep_candidate', $6)",
                &[
                    &Uuid::new_v4(),
                    &c1,
                    &source,
                    &Uuid::new_v4(),
                    &c2,
                    &library,
                ],
            )
            .await
            .unwrap();
        let error = MigrationRunner::run_pending(&mut client)
            .await
            .expect_err("library conflict");
        assert!(error.to_string().contains("import/library pair"));
        assert!(error
            .to_string()
            .contains("conflicting normalized review outcomes"));
        assert_eq!(
            client
                .query_one("SELECT COUNT(*) FROM duplicate_candidates", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            2
        );
        assert_eq!(
            client
                .query_one("SELECT COUNT(*) FROM review_decisions", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            2
        );
        assert_eq!(
            MigrationRunner::current_version(&client)
                .await
                .unwrap()
                .as_deref(),
            Some("0012_album_workflow_repair")
        );
        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
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
        use crate::repositories::import_repository::ImportRepository;
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
            let id = Uuid::new_v4();
            let source_path = format!("C:/migration-0012/{marker}.png");
            let relative_path = format!("album/{marker}.png");
            let hash = vec![marker; 8];
            let blake3 = vec![marker; 32];
            client
                .execute(
                    "INSERT INTO import_images (
                        id, import_album_id, source_path, relative_path, file_size,
                        width, height, format, decode_state, blake3, pixel_hash,
                        gradient_hash, block_hash, median_hash, fingerprint_version, state
                     ) VALUES (
                        $1, $2, $3, $4, 10, 1, 1, 'png', 'decoded', $5, $6,
                        $6, $6, $6, 'test', 'fingerprinted'
                     )",
                    &[&id, &album_id, &source_path, &relative_path, &blake3, &hash],
                )
                .await
                .unwrap();
            id
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
        client
            .execute(
                "INSERT INTO duplicate_candidates (
                    id, import_run_id, source_image_id, candidate_source_image_id,
                    scope, match_type, blake3_equal, pixel_hash_equal,
                    confidence, decision, decision_source
                 ) VALUES ($1, $2, $3, $4, 'cross_album', 'file_exact', TRUE, FALSE,
                           1.0, 'auto_duplicate', 'exact_rule')",
                &[
                    &Uuid::new_v4(),
                    &import_run_id,
                    &stale_image,
                    &terminal_image,
                ],
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
        assert_eq!(
            applied,
            vec![
                "0012_album_workflow_repair",
                "0013_workflow_escape_and_candidate_uniqueness",
                "0014_candidate_review_semantics_and_abandoned_filters",
                "0015_fingerprint_v2"
            ]
        );

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
            Some(MigrationRunner::latest_version().to_string())
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

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_migration_0015_rejects_incomplete_v2_rows_and_drops_old_exact_indexes() {
        use crate::repositories::import_repository::ImportRepository;
        use tempfile::TempDir;
        use uuid::Uuid;

        let tmp = TempDir::new().unwrap();
        let mut manager = crate::infrastructure::postgres::PostgresManager::new(tmp.path());
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);
        let (mut client, handle) = manager.connect_raw().await.unwrap();
        MigrationRunner::run_pending(&mut client).await.unwrap();

        let library_root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        let import_run_id = ImportRepository::create_import_run(
            &client,
            "C:/migration-0015/source",
            library_root_id,
        )
        .await
        .unwrap();
        let import_album_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            "C:/migration-0015/source/album",
            "album",
        )
        .await
        .unwrap();
        let library_album_id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO library_albums
                    (id, library_root_id, display_name, relative_path, manifest_version,
                     manifest_hash, image_count, state)
                 VALUES ($1, $2, 'album', $3, '1', $4, 0, 'committed')",
                &[
                    &library_album_id,
                    &library_root_id,
                    &format!("migration-0015-{library_album_id}"),
                    &vec![1_u8; 32],
                ],
            )
            .await
            .unwrap();

        for missing in ["pixel_hash", "block_hash_16", "double_gradient_hash_32"] {
            let image_id = Uuid::new_v4();
            let source_path = format!("C:/migration-0015/{image_id}.png");
            let relative_path = format!("album/{image_id}.png");
            let blake3 = Some(vec![1_u8; 32]);
            let pixel_hash = (missing != "pixel_hash").then(|| vec![2_u8; 32]);
            let block_hash = (missing != "block_hash_16").then(|| vec![3_u8; 32]);
            let fine_hash = (missing != "double_gradient_hash_32").then(|| vec![4_u8; 68]);
            let error = client
                .execute(
                    "INSERT INTO import_images
                        (id, import_album_id, source_path, relative_path, file_size,
                         width, height, format, decode_state, blake3, pixel_hash,
                         block_hash_16, double_gradient_hash_32, fingerprint_version, state)
                     VALUES ($1, $2, $3, $4, 1, 1, 1, 'png', 'decoded',
                             $5, $6, $7, $8, '2', 'fingerprinted')",
                    &[
                        &image_id,
                        &import_album_id,
                        &source_path,
                        &relative_path,
                        &blake3,
                        &pixel_hash,
                        &block_hash,
                        &fine_hash,
                    ],
                )
                .await
                .expect_err("incomplete import Fingerprint V2 must be rejected");
            assert_eq!(
                error.as_db_error().and_then(|db| db.constraint()),
                Some("chk_import_images_fingerprint_v2_lengths")
            );

            let library_image_id = Uuid::new_v4();
            let library_relative_path = format!("{library_image_id}.png");
            let error = client
                .execute(
                    "INSERT INTO library_images
                        (id, album_id, relative_path, file_size, width, height, format,
                         blake3, pixel_hash, block_hash_16, double_gradient_hash_32,
                         fingerprint_version, state)
                     VALUES ($1, $2, $3, 1, 1, 1, 'png', $4, $5, $6, $7, '2', 'committed')",
                    &[
                        &library_image_id,
                        &library_album_id,
                        &library_relative_path,
                        &blake3,
                        &pixel_hash,
                        &block_hash,
                        &fine_hash,
                    ],
                )
                .await
                .expect_err("incomplete library Fingerprint V2 must be rejected");
            assert_eq!(
                error.as_db_error().and_then(|db| db.constraint()),
                Some("chk_library_images_fingerprint_v2_lengths")
            );
        }

        let old_index_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM pg_indexes
                 WHERE schemaname = 'public'
                   AND indexname IN (
                       'idx_import_images_blake3', 'idx_import_images_pixel_hash',
                       'idx_library_images_blake3', 'idx_library_images_pixel_hash'
                   )",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(old_index_count, 0);
        let v2_index_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM pg_indexes
                 WHERE schemaname = 'public'
                   AND indexname IN (
                       'idx_import_images_blake3_v2', 'idx_import_images_pixel_hash_v2',
                       'idx_library_images_blake3_v2', 'idx_library_images_pixel_hash_v2'
                   )",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(v2_index_count, 4);

        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
    }
}
