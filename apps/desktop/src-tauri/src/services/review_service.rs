use crate::domain::import_state::{
    ImportPlan, ImportPlanImage, ReviewCandidateDetail, ReviewCandidateSummary,
    ReviewDecisionAction, ReviewProgress,
};
use crate::error::AppError;
use crate::repositories::import_repository::{
    AlbumRow, ImportPlanCandidateRow, ImportPlanImageRow, ImportRepository,
};
use base64::Engine;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tokio_postgres::Client;
use uuid::Uuid;

pub async fn get_review_queue(
    client: &Client,
    import_run_id: Uuid,
) -> Result<Vec<ReviewCandidateSummary>, AppError> {
    let rows = ImportRepository::get_review_candidates(client, import_run_id).await?;

    Ok(rows
        .into_iter()
        .map(|r| ReviewCandidateSummary {
            candidate_id: r.candidate_id.to_string(),
            source_image_id: r.source_image_id.to_string(),
            candidate_source_image_id: r.candidate_source_image_id.map(|id| id.to_string()),
            candidate_library_image_id: r.candidate_library_image_id.map(|id| id.to_string()),
            scope: r.scope,
            match_type: r.match_type,
            transform_type: r.transform_type,
            confidence: r.confidence,
            album_name: r.album_name,
            has_decision: r.has_decision,
        })
        .collect())
}

pub async fn get_review_detail(
    client: &Client,
    candidate_id: Uuid,
) -> Result<ReviewCandidateDetail, AppError> {
    let row = ImportRepository::get_review_candidate_detail(client, candidate_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("candidate {candidate_id} not found")))?;

    Ok(ReviewCandidateDetail {
        candidate_id: row.candidate_id.to_string(),
        source_image_id: row.source_image_id.to_string(),
        source_image_path: row.source_image_path,
        source_image_file_size: row.source_image_file_size,
        source_image_width: row.source_image_width,
        source_image_height: row.source_image_height,
        candidate_source_image_id: row.candidate_source_image_id.map(|id| id.to_string()),
        candidate_source_image_path: row.candidate_source_image_path,
        candidate_source_image_file_size: row.candidate_source_image_file_size,
        candidate_source_image_width: row.candidate_source_image_width,
        candidate_source_image_height: row.candidate_source_image_height,
        candidate_library_image_id: row.candidate_library_image_id.map(|id| id.to_string()),
        candidate_library_image_path: row.candidate_library_image_path,
        candidate_library_image_file_size: row.candidate_library_image_file_size,
        candidate_library_image_width: row.candidate_library_image_width,
        candidate_library_image_height: row.candidate_library_image_height,
        scope: row.scope,
        match_type: row.match_type,
        blake3_equal: row.blake3_equal,
        pixel_hash_equal: row.pixel_hash_equal,
        gradient_distance: row.gradient_distance,
        block_distance: row.block_distance,
        median_distance: row.median_distance,
        transform_type: row.transform_type,
        confidence: row.confidence,
        album_name: row.album_name,
        album_id: row.album_id.to_string(),
        existing_decision: row.existing_decision,
    })
}

pub async fn submit_decision(
    client: &Client,
    candidate_id: Uuid,
    action: ReviewDecisionAction,
) -> Result<(), AppError> {
    let row = ImportRepository::get_review_candidate_detail(client, candidate_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("candidate {candidate_id} not found")))?;

    let selected_image_id = match action {
        ReviewDecisionAction::KeepSource => Some(row.source_image_id),
        ReviewDecisionAction::KeepCandidate => row
            .candidate_source_image_id
            .or(row.candidate_library_image_id),
        ReviewDecisionAction::KeepAll | ReviewDecisionAction::SkipAlbum => None,
    };

    let decision_str = action.to_string();
    ImportRepository::insert_review_decision_once(
        client,
        candidate_id,
        &decision_str,
        selected_image_id,
        None,
    )
    .await
}

pub async fn skip_album_candidates(
    client: &Client,
    import_run_id: Uuid,
    album_id: Uuid,
) -> Result<u32, AppError> {
    let candidates = ImportRepository::get_review_candidates(client, import_run_id).await?;
    let mut count = 0u32;

    for c in &candidates {
        if c.has_decision {
            continue;
        }
        let detail = ImportRepository::get_review_candidate_detail(client, c.candidate_id).await?;
        if let Some(d) = detail {
            if d.album_id == album_id {
                ImportRepository::insert_review_decision_once(
                    client,
                    c.candidate_id,
                    &ReviewDecisionAction::SkipAlbum.to_string(),
                    None,
                    Some("album skipped"),
                )
                .await?;
                count += 1;
            }
        }
    }

    Ok(count)
}

pub async fn get_review_progress(
    client: &Client,
    import_run_id: Uuid,
) -> Result<ReviewProgress, AppError> {
    let row = ImportRepository::get_review_progress(client, import_run_id).await?;
    let remaining = row.total.saturating_sub(row.decided);

    Ok(ReviewProgress {
        import_run_id: import_run_id.to_string(),
        total_review_candidates: row.total,
        decided_count: row.decided,
        remaining_count: remaining,
        all_decided: remaining == 0 && row.total > 0,
    })
}

pub async fn generate_import_plan(
    client: &Client,
    import_run_id: Uuid,
) -> Result<ImportPlan, AppError> {
    let progress = ImportRepository::get_review_progress(client, import_run_id).await?;
    let remaining = progress.total.saturating_sub(progress.decided);
    if remaining > 0 {
        return Err(AppError::Internal(format!(
            "cannot generate import plan while {remaining} review candidates remain undecided"
        )));
    }

    let all_images =
        ImportRepository::get_all_import_images_with_album(client, import_run_id).await?;
    let all_candidates =
        ImportRepository::get_all_candidates_for_import_plan(client, import_run_id).await?;
    let albums = ImportRepository::get_albums_for_run(client, import_run_id).await?;

    let plan = build_import_plan(
        import_run_id.to_string(),
        &all_images,
        &all_candidates,
        &albums,
    );

    Ok(plan)
}

pub fn build_import_plan(
    import_run_id: String,
    all_images: &[ImportPlanImageRow],
    all_candidates: &[ImportPlanCandidateRow],
    albums: &[AlbumRow],
) -> ImportPlan {
    let mut excluded_image_ids: HashSet<Uuid> = HashSet::new();
    let mut skipped_album_ids: HashSet<Uuid> = HashSet::new();

    let album_name_map: HashMap<Uuid, String> = albums
        .iter()
        .map(|a: &AlbumRow| (a.id, a.source_name.clone()))
        .collect();

    for c in all_candidates {
        match (
            c.candidate_decision.as_deref(),
            c.review_decision.as_deref(),
        ) {
            (Some("auto_duplicate"), _) => {
                excluded_image_ids.insert(c.source_image_id);
            }
            (None, Some(review_decision)) => match review_decision {
                "keep_source" => {
                    if c.scope == "intra_album" {
                        if let Some(cid) = c.candidate_source_image_id {
                            excluded_image_ids.insert(cid);
                        }
                    }
                }
                "keep_candidate" => {
                    if c.scope == "intra_album" || c.scope == "library" {
                        excluded_image_ids.insert(c.source_image_id);
                    }
                }
                "keep_all" => {}
                "skip_album" => {
                    skipped_album_ids.insert(c.source_album_id);
                }
                _ => {}
            },
            (None, None) => {}
            _ => {}
        }
    }

    let kept_images: Vec<ImportPlanImage> = all_images
        .iter()
        .filter(|img| {
            !excluded_image_ids.contains(&img.id) && !skipped_album_ids.contains(&img.album_id)
        })
        .map(|img: &ImportPlanImageRow| ImportPlanImage {
            image_id: img.id.to_string(),
            source_path: img.source_path.clone(),
            relative_path: img.relative_path.clone(),
            file_size: img.file_size,
            album_name: img.album_name.clone(),
        })
        .collect();

    let total_images = all_images.len() as u32;
    let skipped_album_names: Vec<String> = skipped_album_ids
        .iter()
        .filter_map(|id| album_name_map.get(id).cloned())
        .collect();

    ImportPlan {
        import_run_id,
        total_albums: albums.len() as u32,
        total_images,
        excluded_count: total_images.saturating_sub(kept_images.len() as u32),
        kept_images,
        skipped_albums: skipped_album_names,
    }
}

pub fn load_image_preview(path: &Path) -> Result<String, AppError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };

    let data = std::fs::read(path)
        .map_err(|e| AppError::IoError(format!("failed to read image {}: {e}", path.display())))?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    let data_url = format!("data:{mime};base64,{b64}");

    Ok(data_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_decision_action_display_parse() {
        let actions = [
            ReviewDecisionAction::KeepSource,
            ReviewDecisionAction::KeepCandidate,
            ReviewDecisionAction::KeepAll,
            ReviewDecisionAction::SkipAlbum,
        ];
        for a in actions {
            assert_eq!(ReviewDecisionAction::from_str_opt(&a.to_string()), Some(a));
        }
    }

    #[test]
    fn review_decision_rejects_unknown() {
        assert_eq!(ReviewDecisionAction::from_str_opt("unknown"), None);
        assert_eq!(ReviewDecisionAction::from_str_opt(""), None);
    }

    fn make_image(id: Uuid, album_id: Uuid, name: &str) -> ImportPlanImageRow {
        ImportPlanImageRow {
            id,
            source_path: format!("/src/{name}"),
            relative_path: name.to_string(),
            file_size: 1000,
            album_id,
            album_name: "album_a".to_string(),
        }
    }

    fn make_album(id: Uuid, name: &str) -> AlbumRow {
        AlbumRow {
            id,
            source_name: name.to_string(),
        }
    }

    #[test]
    fn plan_excludes_auto_duplicates() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_b,
            candidate_source_image_id: Some(img_a),
            scope: "intra_album".to_string(),
            candidate_decision: Some("auto_duplicate".to_string()),
            review_decision: None,
            source_album_id: album_id,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, img_a.to_string());
        assert_eq!(plan.excluded_count, 1);
    }

    #[test]
    fn plan_keep_source_excludes_candidate_intra_album() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_source".to_string()),
            source_album_id: album_id,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, img_a.to_string());
    }

    #[test]
    fn plan_keep_candidate_excludes_source_intra_album() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_candidate".to_string()),
            source_album_id: album_id,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, img_b.to_string());
    }

    #[test]
    fn plan_keep_all_keeps_both() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_all".to_string()),
            source_album_id: album_id,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 2);
        assert_eq!(plan.excluded_count, 0);
    }

    #[test]
    fn plan_skip_album_excludes_all_images_in_album() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let img_c = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
            make_image(img_c, album_id, "c.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("skip_album".to_string()),
            source_album_id: album_id,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 0);
        assert_eq!(plan.excluded_count, 3);
        assert_eq!(plan.skipped_albums, vec!["album_a".to_string()]);
    }

    #[test]
    fn plan_library_scope_keep_source_does_not_exclude_library() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![make_image(img_a, album_id, "a.jpg")];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: None,
            scope: "library".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_source".to_string()),
            source_album_id: album_id,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.excluded_count, 0);
    }

    #[test]
    fn plan_library_scope_keep_candidate_excludes_source() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![make_image(img_a, album_id, "a.jpg")];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: None,
            scope: "library".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_candidate".to_string()),
            source_album_id: album_id,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 0);
        assert_eq!(plan.excluded_count, 1);
    }

    #[test]
    fn plan_undecided_review_candidate_not_excluded() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: None,
            source_album_id: album_id,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 2);
    }

    #[test]
    fn plan_empty_run() {
        let plan = build_import_plan("run-1".to_string(), &[], &[], &[]);
        assert_eq!(plan.kept_images.len(), 0);
        assert_eq!(plan.total_albums, 0);
        assert_eq!(plan.total_images, 0);
        assert_eq!(plan.excluded_count, 0);
    }

    /// Real PostgreSQL review integration test.
    ///
    /// Invocation:
    ///   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test \
    ///       --manifest-path apps/desktop/src-tauri/Cargo.toml \
    ///       --features real-db-tests real_review_decision_persists_and_filters_plan \
    ///       -- --ignored --test-threads=1
    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_review_decision_persists_and_filters_plan() {
        use crate::domain::import_state::{
            DecodeState, DuplicateScope, ImportImageState, ImportRunState, MatchType,
        };
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
        use crate::repositories::import_repository::{NewDuplicateCandidate, NewImportImage};
        use tempfile::TempDir;

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            eprintln!("IMAGEDB_POSTGRES_BIN not set; skipping real review integration test");
            return;
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let album_path = source_root.join("album_a");
        std::fs::create_dir_all(&album_path).unwrap();
        std::fs::write(album_path.join("source.png"), b"source").unwrap();
        std::fs::write(album_path.join("candidate.png"), b"candidate").unwrap();

        let mut manager = PostgresManager::new(&app_data);
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);
        assert!(
            probe.pgvector_available,
            "diagnostics: {:?}",
            probe.diagnostics
        );

        let (mut client, db_handle) = manager.connect().await.unwrap();
        MigrationRunner::run_pending(&mut client).await.unwrap();

        let library_root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        let import_run_id = ImportRepository::create_import_run(
            &client,
            &source_root.display().to_string(),
            library_root_id,
        )
        .await
        .unwrap();
        let album_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            &album_path.display().to_string(),
            "album_a",
        )
        .await
        .unwrap();

        let source_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id,
                source_path: album_path.join("source.png").display().to_string(),
                relative_path: "album_a/source.png".to_string(),
                file_size: 6,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(vec![1; 32]),
                pixel_hash: Some(vec![1; 8]),
                gradient_hash: Some(vec![1; 8]),
                block_hash: Some(vec![1; 8]),
                median_hash: Some(vec![1; 8]),
                fingerprint_version: Some("test".to_string()),
                state: ImportImageState::Fingerprinted,
            },
        )
        .await
        .unwrap();
        let candidate_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id,
                source_path: album_path.join("candidate.png").display().to_string(),
                relative_path: "album_a/candidate.png".to_string(),
                file_size: 9,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(vec![2; 32]),
                pixel_hash: Some(vec![2; 8]),
                gradient_hash: Some(vec![2; 8]),
                block_hash: Some(vec![2; 8]),
                median_hash: Some(vec![2; 8]),
                fingerprint_version: Some("test".to_string()),
                state: ImportImageState::Fingerprinted,
            },
        )
        .await
        .unwrap();

        let review_candidate_id = ImportRepository::insert_duplicate_candidate(
            &client,
            NewDuplicateCandidate {
                import_run_id,
                source_image_id: source_id,
                candidate_source_image_id: Some(candidate_id),
                candidate_library_image_id: None,
                scope: DuplicateScope::IntraAlbum,
                match_type: MatchType::PerceptualSimilar,
                blake3_equal: false,
                pixel_hash_equal: false,
                gradient_distance: Some(10),
                block_distance: Some(11),
                median_distance: Some(12),
                transform_type: Some("identity".to_string()),
                confidence: Some(0.75),
                decision: None,
                decision_source: None,
            },
        )
        .await
        .unwrap();

        ImportRepository::update_import_run_state(
            &client,
            import_run_id,
            &ImportRunState::Completed,
        )
        .await
        .unwrap();

        let queue = get_review_queue(&client, import_run_id).await.unwrap();
        assert_eq!(queue.len(), 1);
        assert!(!queue[0].has_decision);

        let blocked_plan = generate_import_plan(&client, import_run_id).await;
        assert!(blocked_plan.is_err());

        submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepSource,
        )
        .await
        .unwrap();
        submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepSource,
        )
        .await
        .unwrap();
        let conflicting = submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepCandidate,
        )
        .await;
        assert!(conflicting.is_err());

        let progress = get_review_progress(&client, import_run_id).await.unwrap();
        assert_eq!(progress.total_review_candidates, 1);
        assert_eq!(progress.decided_count, 1);
        assert!(progress.all_decided);

        let queue = get_review_queue(&client, import_run_id).await.unwrap();
        assert_eq!(queue.len(), 1);
        assert!(queue[0].has_decision);

        let plan = generate_import_plan(&client, import_run_id).await.unwrap();
        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, source_id.to_string());
        assert_eq!(plan.excluded_count, 1);

        drop(client);
        db_handle.abort();
        manager.shutdown().await.unwrap();
    }
}
