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
}
