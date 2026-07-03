#![allow(dead_code)]

use crate::domain::import_state::{
    Decision, DecisionSource, DecodeState, DuplicateScope, ImportAlbumState, ImportImageState,
    ImportRunState, MatchType, SCAN_POLICY_VERSION,
};
use crate::error::AppError;
use tokio_postgres::Client;
use uuid::Uuid;

pub struct ImportRepository;

pub struct ImportRunRecord {
    pub id: Uuid,
    pub source_root: String,
    pub library_root_id: Uuid,
    pub state: String,
    pub policy_version: String,
    pub statistics: serde_json::Value,
}

pub struct ImportAlbumRecord {
    pub id: Uuid,
    pub source_path: String,
    pub source_name: String,
    pub state: String,
}

pub struct ImportImageRecord {
    pub id: Uuid,
    pub source_path: String,
    pub relative_path: String,
    pub file_size: i64,
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
    pub decode_state: String,
    pub blake3: Option<Vec<u8>>,
    pub pixel_hash: Option<Vec<u8>>,
    pub fingerprint_version: Option<String>,
    pub state: String,
}

pub struct LibraryImageRow {
    pub id: Uuid,
    pub file_size: i64,
    pub blake3: Vec<u8>,
    pub pixel_hash: Option<Vec<u8>>,
    pub gradient_hash: Option<Vec<u8>>,
    pub block_hash: Option<Vec<u8>>,
    pub median_hash: Option<Vec<u8>>,
}

pub struct NewImportImage {
    pub album_id: Uuid,
    pub source_path: String,
    pub relative_path: String,
    pub file_size: i64,
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
    pub decode_state: DecodeState,
    pub blake3: Option<Vec<u8>>,
    pub pixel_hash: Option<Vec<u8>>,
    pub gradient_hash: Option<Vec<u8>>,
    pub block_hash: Option<Vec<u8>>,
    pub median_hash: Option<Vec<u8>>,
    pub fingerprint_version: Option<String>,
    pub state: ImportImageState,
}

pub struct NewDuplicateCandidate {
    pub import_run_id: Uuid,
    pub source_image_id: Uuid,
    pub candidate_source_image_id: Option<Uuid>,
    pub candidate_library_image_id: Option<Uuid>,
    pub scope: DuplicateScope,
    pub match_type: MatchType,
    pub blake3_equal: bool,
    pub pixel_hash_equal: bool,
    pub gradient_distance: Option<i32>,
    pub block_distance: Option<i32>,
    pub median_distance: Option<i32>,
    pub transform_type: Option<String>,
    pub confidence: Option<f64>,
    pub decision: Option<Decision>,
    pub decision_source: Option<DecisionSource>,
}

pub struct ReviewCandidateRow {
    pub candidate_id: Uuid,
    pub source_image_id: Uuid,
    pub candidate_source_image_id: Option<Uuid>,
    pub candidate_library_image_id: Option<Uuid>,
    pub scope: String,
    pub match_type: String,
    pub transform_type: Option<String>,
    pub confidence: Option<f64>,
    pub album_name: String,
    pub has_decision: bool,
}

pub struct ReviewCandidateDetailRow {
    pub candidate_id: Uuid,
    pub source_image_id: Uuid,
    pub source_image_path: String,
    pub source_image_file_size: i64,
    pub source_image_width: Option<i32>,
    pub source_image_height: Option<i32>,
    pub candidate_source_image_id: Option<Uuid>,
    pub candidate_source_image_path: Option<String>,
    pub candidate_source_image_file_size: Option<i64>,
    pub candidate_source_image_width: Option<i32>,
    pub candidate_source_image_height: Option<i32>,
    pub candidate_library_image_id: Option<Uuid>,
    pub candidate_library_image_path: Option<String>,
    pub candidate_library_image_file_size: Option<i64>,
    pub candidate_library_image_width: Option<i32>,
    pub candidate_library_image_height: Option<i32>,
    pub scope: String,
    pub match_type: String,
    pub blake3_equal: bool,
    pub pixel_hash_equal: bool,
    pub gradient_distance: Option<i32>,
    pub block_distance: Option<i32>,
    pub median_distance: Option<i32>,
    pub transform_type: Option<String>,
    pub confidence: Option<f64>,
    pub album_name: String,
    pub album_id: Uuid,
    pub existing_decision: Option<String>,
}

pub struct ReviewProgressRow {
    pub total: u32,
    pub decided: u32,
}

pub struct ImportPlanCandidateRow {
    pub candidate_id: Uuid,
    pub source_image_id: Uuid,
    pub candidate_source_image_id: Option<Uuid>,
    pub scope: String,
    pub candidate_decision: Option<String>,
    pub review_decision: Option<String>,
    pub source_album_id: Uuid,
}

pub struct ImportPlanImageRow {
    pub id: Uuid,
    pub source_path: String,
    pub relative_path: String,
    pub file_size: i64,
    pub album_id: Uuid,
    pub album_name: String,
}

pub struct AlbumRow {
    pub id: Uuid,
    pub source_name: String,
}

pub struct ImportAlbumFullRow {
    pub id: Uuid,
    pub source_path: String,
    pub source_name: String,
}

pub struct ImportImageFullRow {
    pub id: Uuid,
    pub source_path: String,
    pub relative_path: String,
    pub file_size: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
    pub blake3: Option<Vec<u8>>,
    pub pixel_hash: Option<Vec<u8>>,
    pub gradient_hash: Option<Vec<u8>>,
    pub block_hash: Option<Vec<u8>>,
    pub median_hash: Option<Vec<u8>>,
    pub fingerprint_version: Option<String>,
    pub import_album_id: Uuid,
}

pub struct LibraryAlbumRow {
    pub id: Uuid,
    pub image_count: i32,
    pub state: String,
}

pub struct FileTransactionRow {
    pub id: Uuid,
    pub state: String,
}

impl ImportRepository {
    pub async fn upsert_default_library_root(client: &Client) -> Result<Uuid, AppError> {
        if let Some(row) = client
            .query_opt(
                "SELECT id FROM library_roots
                 WHERE display_name = '_default_'
                 ORDER BY created_at
                 LIMIT 1",
                &[],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query default library root: {e}")))?
        {
            return Ok(row.get("id"));
        }

        let id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO library_roots (id, path, display_name, is_active)
                 VALUES ($1, '', '_default_', TRUE)",
                &[&id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to insert default library root: {e}"))
            })?;

        Ok(id)
    }

    pub async fn create_import_run(
        client: &Client,
        source_root: &str,
        library_root_id: Uuid,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let state = ImportRunState::Scanning.to_string();
        client
            .execute(
                "INSERT INTO import_runs (id, source_root, library_root_id, state, policy_version, statistics)
                 VALUES ($1, $2, $3, $4, $5, '{}'::jsonb)",
                &[&id, &source_root, &library_root_id, &state, &SCAN_POLICY_VERSION.to_string()],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to create import run: {e}")))?;
        Ok(id)
    }

    pub async fn update_import_run_state(
        client: &Client,
        id: Uuid,
        state: &ImportRunState,
    ) -> Result<(), AppError> {
        let state_str = state.to_string();
        let completed_at = match state {
            ImportRunState::Completed | ImportRunState::Cancelled | ImportRunState::Failed => {
                Some(chrono::Utc::now())
            }
            _ => None,
        };
        client
            .execute(
                "UPDATE import_runs SET state = $1, completed_at = COALESCE($2, completed_at) WHERE id = $3",
                &[&state_str, &completed_at, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to update import run state: {e}")))?;
        Ok(())
    }

    pub async fn update_import_run_error(
        client: &Client,
        id: Uuid,
        error_code: &str,
        error_message: &str,
    ) -> Result<(), AppError> {
        let state = ImportRunState::Failed.to_string();
        client
            .execute(
                "UPDATE import_runs SET state = $1, error_code = $2, error_message = $3, completed_at = now() WHERE id = $4",
                &[&state, &error_code, &error_message, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to update import run error: {e}")))?;
        Ok(())
    }

    pub async fn update_import_run_statistics(
        client: &Client,
        id: Uuid,
        statistics: &serde_json::Value,
    ) -> Result<(), AppError> {
        client
            .execute(
                "UPDATE import_runs SET statistics = $1 WHERE id = $2",
                &[statistics, &id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to update import run statistics: {e}"))
            })?;
        Ok(())
    }

    pub async fn insert_import_album(
        client: &Client,
        import_run_id: Uuid,
        source_path: &str,
        source_name: &str,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let state = ImportAlbumState::Pending.to_string();
        client
            .execute(
                "INSERT INTO import_albums (id, import_run_id, source_path, source_name, state)
                 VALUES ($1, $2, $3, $4, $5)",
                &[&id, &import_run_id, &source_path, &source_name, &state],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert import album: {e}")))?;
        Ok(id)
    }

    pub async fn update_import_album_state(
        client: &Client,
        id: Uuid,
        state: &ImportAlbumState,
    ) -> Result<(), AppError> {
        let state_str = state.to_string();
        client
            .execute(
                "UPDATE import_albums SET state = $1 WHERE id = $2",
                &[&state_str, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to update album state: {e}")))?;
        Ok(())
    }

    pub async fn insert_import_image(
        client: &Client,
        new_image: NewImportImage,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let ds = new_image.decode_state.to_string();
        let st = new_image.state.to_string();
        client
            .execute(
                "INSERT INTO import_images
                 (id, import_album_id, source_path, relative_path, file_size, modified_at,
                  width, height, format, decode_state, blake3, pixel_hash,
                  gradient_hash, block_hash, median_hash,
                  fingerprint_version, state)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
                &[
                    &id,
                    &new_image.album_id,
                    &new_image.source_path,
                    &new_image.relative_path,
                    &new_image.file_size,
                    &new_image.modified_at,
                    &new_image.width,
                    &new_image.height,
                    &new_image.format,
                    &ds,
                    &new_image.blake3,
                    &new_image.pixel_hash,
                    &new_image.gradient_hash,
                    &new_image.block_hash,
                    &new_image.median_hash,
                    &new_image.fingerprint_version,
                    &st,
                ],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert import image: {e}")))?;
        Ok(id)
    }

    pub async fn get_import_images_by_album(
        client: &Client,
        album_id: Uuid,
    ) -> Result<Vec<ImportImageRecord>, AppError> {
        let rows = client
            .query(
                "SELECT id, source_path, relative_path, file_size, modified_at,
                        width, height, format, decode_state, blake3, pixel_hash,
                        fingerprint_version, state
                 FROM import_images WHERE import_album_id = $1",
                &[&album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to get images by album: {e}")))?;

        Ok(rows
            .iter()
            .map(|r| ImportImageRecord {
                id: r.get("id"),
                source_path: r.get("source_path"),
                relative_path: r.get("relative_path"),
                file_size: r.get("file_size"),
                modified_at: r.get("modified_at"),
                width: r.get("width"),
                height: r.get("height"),
                format: r.get("format"),
                decode_state: r.get("decode_state"),
                blake3: r.get("blake3"),
                pixel_hash: r.get("pixel_hash"),
                fingerprint_version: r.get("fingerprint_version"),
                state: r.get("state"),
            })
            .collect())
    }

    pub async fn insert_duplicate_candidate(
        client: &Client,
        candidate: NewDuplicateCandidate,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let scope_str = candidate.scope.to_string();
        let match_str = candidate.match_type.to_string();
        let decision_str = candidate.decision.as_ref().map(|d| d.to_string());
        let source_str = candidate.decision_source.as_ref().map(|s| s.to_string());
        client
            .execute(
                "INSERT INTO duplicate_candidates
                 (id, import_run_id, source_image_id, candidate_source_image_id,
                  candidate_library_image_id, scope, match_type,
                  blake3_equal, pixel_hash_equal,
                  gradient_distance, block_distance, median_distance,
                  transform_type, confidence,
                  decision, decision_source, rule_version)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
                &[
                    &id,
                    &candidate.import_run_id,
                    &candidate.source_image_id,
                    &candidate.candidate_source_image_id,
                    &candidate.candidate_library_image_id,
                    &scope_str,
                    &match_str,
                    &candidate.blake3_equal,
                    &candidate.pixel_hash_equal,
                    &candidate.gradient_distance,
                    &candidate.block_distance,
                    &candidate.median_distance,
                    &candidate.transform_type,
                    &candidate.confidence,
                    &decision_str,
                    &source_str,
                    &SCAN_POLICY_VERSION.to_string(),
                ],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to insert duplicate candidate: {e}"))
            })?;
        Ok(id)
    }

    pub async fn get_library_images_for_comparison(
        client: &Client,
    ) -> Result<Vec<LibraryImageRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, file_size, blake3, pixel_hash, gradient_hash, block_hash, median_hash FROM library_images",
                &[],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query library images: {e}")))?;

        Ok(rows
            .iter()
            .map(|r| LibraryImageRow {
                id: r.get("id"),
                file_size: r.get("file_size"),
                blake3: r.get("blake3"),
                pixel_hash: r.get("pixel_hash"),
                gradient_hash: r.get("gradient_hash"),
                block_hash: r.get("block_hash"),
                median_hash: r.get("median_hash"),
            })
            .collect())
    }

    pub async fn count_duplicates_for_run(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<i64, AppError> {
        let row = client
            .query_one(
                "SELECT COUNT(*) FROM duplicate_candidates WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to count duplicates: {e}")))?;
        Ok(row.get(0))
    }

    pub async fn get_review_candidates(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ReviewCandidateRow>, AppError> {
        let rows = client
            .query(
                "SELECT dc.id AS candidate_id,
                        dc.source_image_id,
                        dc.candidate_source_image_id,
                        dc.candidate_library_image_id,
                        dc.scope,
                        dc.match_type,
                        dc.transform_type,
                        dc.confidence,
                        ia.id AS album_id,
                        ia.source_name AS album_name,
                        (rd.id IS NOT NULL) AS has_decision
                 FROM duplicate_candidates dc
                 JOIN import_images si ON dc.source_image_id = si.id
                 JOIN import_albums ia ON si.import_album_id = ia.id
                 LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                 WHERE dc.import_run_id = $1
                   AND dc.decision IS NULL
                 ORDER BY ia.source_name, dc.created_at",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query review candidates: {e}")))?;

        Ok(rows
            .iter()
            .map(|r| ReviewCandidateRow {
                candidate_id: r.get("candidate_id"),
                source_image_id: r.get("source_image_id"),
                candidate_source_image_id: r.get("candidate_source_image_id"),
                candidate_library_image_id: r.get("candidate_library_image_id"),
                scope: r.get("scope"),
                match_type: r.get("match_type"),
                transform_type: r.get("transform_type"),
                confidence: r.get("confidence"),
                album_name: r.get("album_name"),
                has_decision: r.get("has_decision"),
            })
            .collect())
    }

    pub async fn get_review_candidate_detail(
        client: &Client,
        candidate_id: Uuid,
    ) -> Result<Option<ReviewCandidateDetailRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT dc.id AS candidate_id,
                        dc.source_image_id,
                        si.source_path AS source_image_path,
                        si.file_size AS source_image_file_size,
                        si.width AS source_image_width,
                        si.height AS source_image_height,
                        dc.candidate_source_image_id,
                        csi.source_path AS candidate_source_image_path,
                        csi.file_size AS candidate_source_image_file_size,
                        csi.width AS candidate_source_image_width,
                        csi.height AS candidate_source_image_height,
                        dc.candidate_library_image_id,
                        concat_ws('\\', lr.path, NULLIF(la.relative_path, ''), cli.relative_path)
                            AS candidate_library_image_path,
                        cli.file_size AS candidate_library_image_file_size,
                        cli.width AS candidate_library_image_width,
                        cli.height AS candidate_library_image_height,
                        dc.scope,
                        dc.match_type,
                        dc.blake3_equal,
                        dc.pixel_hash_equal,
                        dc.gradient_distance,
                        dc.block_distance,
                        dc.median_distance,
                        dc.transform_type,
                        dc.confidence,
                        ia.source_name AS album_name,
                        ia.id AS album_id,
                        rd.decision AS existing_decision
                 FROM duplicate_candidates dc
                 JOIN import_images si ON dc.source_image_id = si.id
                 JOIN import_albums ia ON si.import_album_id = ia.id
                 LEFT JOIN import_images csi ON dc.candidate_source_image_id = csi.id
                 LEFT JOIN library_images cli ON dc.candidate_library_image_id = cli.id
                 LEFT JOIN library_albums la ON cli.album_id = la.id
                 LEFT JOIN library_roots lr ON la.library_root_id = lr.id
                 LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                 WHERE dc.id = $1",
                &[&candidate_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query review candidate detail: {e}"))
            })?;

        Ok(row.map(|r| ReviewCandidateDetailRow {
            candidate_id: r.get("candidate_id"),
            source_image_id: r.get("source_image_id"),
            source_image_path: r.get("source_image_path"),
            source_image_file_size: r.get("source_image_file_size"),
            source_image_width: r.get("source_image_width"),
            source_image_height: r.get("source_image_height"),
            candidate_source_image_id: r.get("candidate_source_image_id"),
            candidate_source_image_path: r.get("candidate_source_image_path"),
            candidate_source_image_file_size: r.get("candidate_source_image_file_size"),
            candidate_source_image_width: r.get("candidate_source_image_width"),
            candidate_source_image_height: r.get("candidate_source_image_height"),
            candidate_library_image_id: r.get("candidate_library_image_id"),
            candidate_library_image_path: r.get("candidate_library_image_path"),
            candidate_library_image_file_size: r.get("candidate_library_image_file_size"),
            candidate_library_image_width: r.get("candidate_library_image_width"),
            candidate_library_image_height: r.get("candidate_library_image_height"),
            scope: r.get("scope"),
            match_type: r.get("match_type"),
            blake3_equal: r.get("blake3_equal"),
            pixel_hash_equal: r.get("pixel_hash_equal"),
            gradient_distance: r.get("gradient_distance"),
            block_distance: r.get("block_distance"),
            median_distance: r.get("median_distance"),
            transform_type: r.get("transform_type"),
            confidence: r.get("confidence"),
            album_name: r.get("album_name"),
            album_id: r.get("album_id"),
            existing_decision: r.get("existing_decision"),
        }))
    }

    pub async fn get_review_decision(
        client: &Client,
        candidate_id: Uuid,
    ) -> Result<Option<String>, AppError> {
        let row = client
            .query_opt(
                "SELECT decision FROM review_decisions WHERE candidate_id = $1",
                &[&candidate_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query review decision: {e}")))?;

        Ok(row.map(|r| r.get("decision")))
    }

    pub async fn insert_review_decision_once(
        client: &Client,
        candidate_id: Uuid,
        decision: &str,
        selected_image_id: Option<Uuid>,
        notes: Option<&str>,
    ) -> Result<(), AppError> {
        let id = Uuid::new_v4();
        let row = client
            .query_opt(
                "INSERT INTO review_decisions (id, candidate_id, decision, selected_image_id, notes)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (candidate_id) DO UPDATE
                 SET decision = review_decisions.decision
                 WHERE review_decisions.decision = EXCLUDED.decision
                   AND review_decisions.selected_image_id IS NOT DISTINCT FROM EXCLUDED.selected_image_id
                 RETURNING id",
                &[&id, &candidate_id, &decision, &selected_image_id, &notes],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to insert review decision: {e}"))
            })?;

        if row.is_none() {
            return Err(AppError::Internal(format!(
                "candidate {candidate_id} already has a conflicting review decision"
            )));
        }

        Ok(())
    }

    pub async fn get_review_progress(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<ReviewProgressRow, AppError> {
        let total_row = client
            .query_one(
                "SELECT COUNT(*) AS total
                 FROM duplicate_candidates
                 WHERE import_run_id = $1 AND decision IS NULL",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to count review candidates: {e}")))?;
        let total: i64 = total_row.get("total");

        let decided_row = client
            .query_one(
                "SELECT COUNT(*) AS decided
                 FROM review_decisions rd
                 JOIN duplicate_candidates dc ON rd.candidate_id = dc.id
                 WHERE dc.import_run_id = $1 AND dc.decision IS NULL",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to count decided reviews: {e}")))?;
        let decided: i64 = decided_row.get("decided");

        Ok(ReviewProgressRow {
            total: total as u32,
            decided: decided as u32,
        })
    }

    pub async fn get_all_candidates_for_import_plan(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ImportPlanCandidateRow>, AppError> {
        let rows = client
            .query(
                "SELECT dc.id AS candidate_id,
                        dc.source_image_id,
                        dc.candidate_source_image_id,
                        dc.scope,
                        dc.decision AS candidate_decision,
                        rd.decision AS review_decision,
                        si.import_album_id AS source_album_id
                 FROM duplicate_candidates dc
                 JOIN import_images si ON dc.source_image_id = si.id
                 LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                 WHERE dc.import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query candidates for import plan: {e}"))
            })?;

        Ok(rows
            .iter()
            .map(|r| ImportPlanCandidateRow {
                candidate_id: r.get("candidate_id"),
                source_image_id: r.get("source_image_id"),
                candidate_source_image_id: r.get("candidate_source_image_id"),
                scope: r.get("scope"),
                candidate_decision: r.get("candidate_decision"),
                review_decision: r.get("review_decision"),
                source_album_id: r.get("source_album_id"),
            })
            .collect())
    }

    pub async fn get_all_import_images_with_album(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ImportPlanImageRow>, AppError> {
        let rows = client
            .query(
                "SELECT ii.id,
                        ii.source_path,
                        ii.relative_path,
                        ii.file_size,
                        ia.id AS album_id,
                        ia.source_name AS album_name
                 FROM import_images ii
                 JOIN import_albums ia ON ii.import_album_id = ia.id
                 WHERE ia.import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query import images for plan: {e}"))
            })?;

        Ok(rows
            .iter()
            .map(|r| ImportPlanImageRow {
                id: r.get("id"),
                source_path: r.get("source_path"),
                relative_path: r.get("relative_path"),
                file_size: r.get("file_size"),
                album_id: r.get("album_id"),
                album_name: r.get("album_name"),
            })
            .collect())
    }

    pub async fn get_albums_for_run(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<AlbumRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, source_name FROM import_albums WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query albums for run: {e}")))?;

        Ok(rows
            .iter()
            .map(|r| AlbumRow {
                id: r.get("id"),
                source_name: r.get("source_name"),
            })
            .collect())
    }

    pub async fn get_latest_completed_run(client: &Client) -> Result<Option<Uuid>, AppError> {
        let row = client
            .query_opt(
                "SELECT id FROM import_runs
                 WHERE state = 'completed'
                 ORDER BY completed_at DESC
                 LIMIT 1",
                &[],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query latest run: {e}")))?;

        Ok(row.map(|r| r.get("id")))
    }

    pub async fn get_import_run_by_id(
        client: &Client,
        id: Uuid,
    ) -> Result<Option<ImportRunRecord>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, source_root, library_root_id, state, policy_version, statistics
                 FROM import_runs WHERE id = $1",
                &[&id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query import run: {e}")))?;

        Ok(row.map(|r| ImportRunRecord {
            id: r.get("id"),
            source_root: r.get("source_root"),
            library_root_id: r.get("library_root_id"),
            state: r.get("state"),
            policy_version: r.get("policy_version"),
            statistics: r.get("statistics"),
        }))
    }

    pub async fn get_import_albums_with_source_for_run(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ImportAlbumFullRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, source_path, source_name FROM import_albums WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query import albums with source: {e}"))
            })?;

        Ok(rows
            .iter()
            .map(|r| ImportAlbumFullRow {
                id: r.get("id"),
                source_path: r.get("source_path"),
                source_name: r.get("source_name"),
            })
            .collect())
    }

    pub async fn get_import_images_by_ids(
        client: &Client,
        image_ids: &[Uuid],
    ) -> Result<Vec<ImportImageFullRow>, AppError> {
        if image_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = client
            .query(
                "SELECT id, source_path, relative_path, file_size, width, height, format,
                        blake3, pixel_hash, gradient_hash, block_hash, median_hash,
                        fingerprint_version, import_album_id
                 FROM import_images WHERE id = ANY($1)",
                &[&image_ids],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query import images by ids: {e}"))
            })?;

        Ok(rows
            .iter()
            .map(|r| ImportImageFullRow {
                id: r.get("id"),
                source_path: r.get("source_path"),
                relative_path: r.get("relative_path"),
                file_size: r.get("file_size"),
                width: r.get("width"),
                height: r.get("height"),
                format: r.get("format"),
                blake3: r.get("blake3"),
                pixel_hash: r.get("pixel_hash"),
                gradient_hash: r.get("gradient_hash"),
                block_hash: r.get("block_hash"),
                median_hash: r.get("median_hash"),
                fingerprint_version: r.get("fingerprint_version"),
                import_album_id: r.get("import_album_id"),
            })
            .collect())
    }

    pub async fn find_library_album_by_path(
        client: &Client,
        library_root_id: Uuid,
        relative_path: &str,
    ) -> Result<Option<LibraryAlbumRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, image_count, state FROM library_albums
                 WHERE library_root_id = $1 AND relative_path = $2",
                &[&library_root_id, &relative_path],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query library album by path: {e}"))
            })?;

        Ok(row.map(|r| LibraryAlbumRow {
            id: r.get("id"),
            image_count: r.get("image_count"),
            state: r.get("state"),
        }))
    }

    pub async fn find_existing_file_transaction(
        client: &Client,
        import_album_id: Uuid,
    ) -> Result<Option<FileTransactionRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, state FROM file_transactions WHERE import_album_id = $1",
                &[&import_album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query file transaction: {e}")))?;

        Ok(row.map(|r| FileTransactionRow {
            id: r.get("id"),
            state: r.get("state"),
        }))
    }

    pub async fn insert_file_transaction(
        client: &Client,
        import_run_id: Uuid,
        import_album_id: Uuid,
        state: &str,
        staging_path: Option<&str>,
        target_path: Option<&str>,
        manifest_path: Option<&str>,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO file_transactions
                 (id, import_run_id, import_album_id, state, staging_path, target_path, manifest_path)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[
                    &id,
                    &import_run_id,
                    &import_album_id,
                    &state,
                    &staging_path,
                    &target_path,
                    &manifest_path,
                ],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to insert file transaction: {e}"))
            })?;
        Ok(id)
    }

    pub async fn update_file_transaction_state(
        client: &Client,
        id: Uuid,
        state: &str,
        last_error: Option<&str>,
    ) -> Result<(), AppError> {
        let completed_at = match state {
            "source_archived" | "committed" => Some(chrono::Utc::now()),
            _ => None,
        };
        client
            .execute(
                "UPDATE file_transactions SET state = $1, last_error = $2,
                 completed_at = COALESCE($3, completed_at) WHERE id = $4",
                &[&state, &last_error, &completed_at, &id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to update file transaction state: {e}"))
            })?;
        Ok(())
    }

    pub async fn insert_file_operation(
        client: &Client,
        transaction_id: Uuid,
        source_path: &str,
        staging_path: &str,
        target_path: &str,
        expected_size: i64,
        expected_blake3: &[u8],
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO file_operations
                 (id, transaction_id, source_path, staging_path, target_path,
                  expected_size, expected_blake3, state)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending')",
                &[
                    &id,
                    &transaction_id,
                    &source_path,
                    &staging_path,
                    &target_path,
                    &expected_size,
                    &expected_blake3,
                ],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert file operation: {e}")))?;
        Ok(id)
    }

    pub async fn update_file_operation_state(
        client: &Client,
        id: Uuid,
        state: &str,
        actual_blake3: Option<&[u8]>,
        last_error: Option<&str>,
    ) -> Result<(), AppError> {
        client
            .execute(
                "UPDATE file_operations SET state = $1, actual_blake3 = COALESCE($2, actual_blake3),
                 last_error = $3, attempt_count = attempt_count + 1, updated_at = now()
                 WHERE id = $4",
                &[&state, &actual_blake3, &last_error, &id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to update file operation state: {e}"))
            })?;
        Ok(())
    }

    pub async fn update_library_root_path(
        client: &Client,
        id: Uuid,
        path: &str,
    ) -> Result<(), AppError> {
        client
            .execute(
                "UPDATE library_roots SET path = $1, updated_at = now() WHERE id = $2",
                &[&path, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to update library root path: {e}")))?;
        Ok(())
    }
}
