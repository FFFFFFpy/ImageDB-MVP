use crate::domain::import_state::TransformType;
use crate::error::AppError;
use crate::infrastructure::image_fingerprint_v2::{
    hamming_distance, BlockHashVariant, FINGERPRINT_VERSION, MAX_RECALL_CANDIDATES_PER_IMAGE,
};
use crate::repositories::import_repository::LibraryImageRow;
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

#[derive(Debug, Clone)]
struct BkNode {
    hash: Vec<u8>,
    children: BTreeMap<u32, BkNode>,
}

#[derive(Debug, Clone, Default)]
pub struct HammingBkTree {
    root: Option<Box<BkNode>>,
    hash_length: Option<usize>,
    unique_hash_count: usize,
}

impl HammingBkTree {
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn unique_hash_count(&self) -> usize {
        self.unique_hash_count
    }

    pub fn insert(&mut self, hash: Vec<u8>) -> Result<bool, AppError> {
        if hash.is_empty() {
            return Err(AppError::Internal(
                "cannot index an empty perceptual hash".to_string(),
            ));
        }
        if let Some(expected) = self.hash_length {
            if hash.len() != expected {
                return Err(AppError::Internal(format!(
                    "cannot mix BK-tree hash lengths: expected {expected} bytes, got {}",
                    hash.len()
                )));
            }
        } else {
            self.hash_length = Some(hash.len());
        }

        let Some(root) = self.root.as_mut() else {
            self.root = Some(Box::new(BkNode {
                hash,
                children: BTreeMap::new(),
            }));
            self.unique_hash_count = 1;
            return Ok(true);
        };

        let mut node = root.as_mut();
        loop {
            let distance = hamming_distance(&hash, &node.hash)?.raw_distance;
            if distance == 0 {
                return Ok(false);
            }
            match node.children.entry(distance) {
                std::collections::btree_map::Entry::Occupied(entry) => {
                    node = entry.into_mut();
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(BkNode {
                        hash,
                        children: BTreeMap::new(),
                    });
                    self.unique_hash_count += 1;
                    return Ok(true);
                }
            }
        }
    }

    pub fn search(&self, query: &[u8], radius: u32) -> Result<Vec<(Vec<u8>, u32)>, AppError> {
        if let Some(expected) = self.hash_length {
            if query.len() != expected {
                return Err(AppError::Internal(format!(
                    "BK-tree query length mismatch: expected {expected} bytes, got {}",
                    query.len()
                )));
            }
        }
        let mut matches = Vec::new();
        if let Some(root) = &self.root {
            search_node(root, query, radius, &mut matches)?;
        }
        matches.sort_by(|(left_hash, left_distance), (right_hash, right_distance)| {
            left_distance
                .cmp(right_distance)
                .then_with(|| left_hash.cmp(right_hash))
        });
        Ok(matches)
    }
}

fn search_node(
    node: &BkNode,
    query: &[u8],
    radius: u32,
    matches: &mut Vec<(Vec<u8>, u32)>,
) -> Result<(), AppError> {
    let distance = hamming_distance(query, &node.hash)?.raw_distance;
    if distance <= radius {
        matches.push((node.hash.clone(), distance));
    }
    let lower = distance.saturating_sub(radius);
    let upper = distance.saturating_add(radius);
    for (_, child) in node.children.range(lower..=upper) {
        search_node(child, query, radius, matches)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct LibraryFingerprintMetadata {
    pub image_id: Uuid,
    pub file_size: i64,
    pub blake3: Vec<u8>,
    pub pixel_hash: Option<Vec<u8>>,
    pub block_hash_16: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryRecallMatch {
    pub image_id: Uuid,
    pub block_distance: u32,
    pub transform: TransformType,
}

#[derive(Debug, Clone)]
pub struct LibraryRecallResult {
    pub matches: Vec<LibraryRecallMatch>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct LibraryFingerprintIndex {
    pub fingerprint_version: u32,
    pub image_count: usize,
    block_tree: HammingBkTree,
    hash_to_image_ids: HashMap<Vec<u8>, Vec<Uuid>>,
    image_by_id: HashMap<Uuid, LibraryFingerprintMetadata>,
    file_exact: HashMap<(i64, Vec<u8>), Vec<Uuid>>,
    pixel_exact: HashMap<Vec<u8>, Vec<Uuid>>,
}

impl Default for LibraryFingerprintIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl LibraryFingerprintIndex {
    pub fn new() -> Self {
        Self {
            fingerprint_version: FINGERPRINT_VERSION,
            image_count: 0,
            block_tree: HammingBkTree::default(),
            hash_to_image_ids: HashMap::new(),
            image_by_id: HashMap::new(),
            file_exact: HashMap::new(),
            pixel_exact: HashMap::new(),
        }
    }

    pub fn build(rows: &[LibraryImageRow]) -> Result<Self, AppError> {
        let mut index = Self::new();
        for row in rows {
            index.add(row)?;
        }
        Ok(index)
    }

    pub fn add(&mut self, row: &LibraryImageRow) -> Result<bool, AppError> {
        if row.fingerprint_version != FINGERPRINT_VERSION.to_string() {
            return Ok(false);
        }
        let block_hash_16 = row.block_hash_16.clone().ok_or_else(|| {
            AppError::Internal(format!(
                "V2 library image {} is missing BlockHash 16x16",
                row.id
            ))
        })?;
        if row.blake3.len() != 32 || block_hash_16.len() != 32 {
            return Err(AppError::Internal(format!(
                "V2 library image {} has invalid hash lengths",
                row.id
            )));
        }
        if row
            .pixel_hash
            .as_ref()
            .is_some_and(|pixel_hash| pixel_hash.len() != 32)
        {
            return Err(AppError::Internal(format!(
                "V2 library image {} has an invalid pixel hash length",
                row.id
            )));
        }

        if self.image_by_id.contains_key(&row.id) {
            self.remove(row.id)?;
        }
        self.block_tree.insert(block_hash_16.clone())?;
        insert_sorted_unique(
            self.hash_to_image_ids
                .entry(block_hash_16.clone())
                .or_default(),
            row.id,
        );
        insert_sorted_unique(
            self.file_exact
                .entry((row.file_size, row.blake3.clone()))
                .or_default(),
            row.id,
        );
        if let Some(pixel_hash) = &row.pixel_hash {
            insert_sorted_unique(
                self.pixel_exact.entry(pixel_hash.clone()).or_default(),
                row.id,
            );
        }
        self.image_by_id.insert(
            row.id,
            LibraryFingerprintMetadata {
                image_id: row.id,
                file_size: row.file_size,
                blake3: row.blake3.clone(),
                pixel_hash: row.pixel_hash.clone(),
                block_hash_16,
            },
        );
        self.image_count = self.image_by_id.len();
        Ok(true)
    }

    pub fn remove(&mut self, image_id: Uuid) -> Result<bool, AppError> {
        if self.image_by_id.remove(&image_id).is_none() {
            return Ok(false);
        }
        self.rebuild_secondary_indexes()?;
        Ok(true)
    }

    fn rebuild_secondary_indexes(&mut self) -> Result<(), AppError> {
        self.block_tree = HammingBkTree::default();
        self.hash_to_image_ids.clear();
        self.file_exact.clear();
        self.pixel_exact.clear();
        let mut rows: Vec<_> = self.image_by_id.values().cloned().collect();
        rows.sort_by_key(|metadata| metadata.image_id);
        for metadata in rows {
            self.block_tree.insert(metadata.block_hash_16.clone())?;
            insert_sorted_unique(
                self.hash_to_image_ids
                    .entry(metadata.block_hash_16.clone())
                    .or_default(),
                metadata.image_id,
            );
            insert_sorted_unique(
                self.file_exact
                    .entry((metadata.file_size, metadata.blake3.clone()))
                    .or_default(),
                metadata.image_id,
            );
            if let Some(pixel_hash) = metadata.pixel_hash {
                insert_sorted_unique(
                    self.pixel_exact.entry(pixel_hash).or_default(),
                    metadata.image_id,
                );
            }
        }
        self.image_count = self.image_by_id.len();
        Ok(())
    }

    pub fn exact_file_matches(&self, file_size: i64, blake3: &[u8]) -> Vec<Uuid> {
        self.file_exact
            .get(&(file_size, blake3.to_vec()))
            .cloned()
            .unwrap_or_default()
    }

    pub fn exact_pixel_matches(&self, pixel_hash: &[u8]) -> Vec<Uuid> {
        self.pixel_exact
            .get(pixel_hash)
            .cloned()
            .unwrap_or_default()
    }

    pub fn recall(
        &self,
        variants: &[BlockHashVariant],
        radius: u32,
    ) -> Result<LibraryRecallResult, AppError> {
        let mut best_by_image: HashMap<Uuid, LibraryRecallMatch> = HashMap::new();
        for variant in variants {
            for (hash, distance) in self.block_tree.search(&variant.hash, radius)? {
                let Some(image_ids) = self.hash_to_image_ids.get(&hash) else {
                    continue;
                };
                for image_id in image_ids {
                    let candidate = LibraryRecallMatch {
                        image_id: *image_id,
                        block_distance: distance,
                        transform: variant.transform,
                    };
                    let should_replace = best_by_image
                        .get(image_id)
                        .map(|current| {
                            (candidate.block_distance, candidate.transform.to_string())
                                < (current.block_distance, current.transform.to_string())
                        })
                        .unwrap_or(true);
                    if should_replace {
                        best_by_image.insert(*image_id, candidate);
                    }
                }
            }
        }
        let mut matches: Vec<_> = best_by_image.into_values().collect();
        matches.sort_by(|left, right| {
            left.block_distance
                .cmp(&right.block_distance)
                .then_with(|| left.image_id.cmp(&right.image_id))
        });
        let truncated = matches.len() > MAX_RECALL_CANDIDATES_PER_IMAGE;
        matches.truncate(MAX_RECALL_CANDIDATES_PER_IMAGE);
        Ok(LibraryRecallResult { matches, truncated })
    }

    pub fn metadata(&self, image_id: Uuid) -> Option<&LibraryFingerprintMetadata> {
        self.image_by_id.get(&image_id)
    }

    pub fn unique_hash_count(&self) -> usize {
        self.block_tree.unique_hash_count()
    }
}

fn insert_sorted_unique(values: &mut Vec<Uuid>, value: Uuid) {
    match values.binary_search(&value) {
        Ok(_) => {}
        Err(position) => values.insert(position, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: Uuid, block: Vec<u8>, marker: u8) -> LibraryImageRow {
        LibraryImageRow {
            id,
            file_size: marker as i64,
            blake3: vec![marker; 32],
            pixel_hash: Some(vec![marker; 32]),
            block_hash_16: Some(block),
            double_gradient_hash_32: Some(vec![marker; 68]),
            fingerprint_version: "2".to_string(),
        }
    }

    fn variants(hash: Vec<u8>) -> Vec<BlockHashVariant> {
        TransformType::ALL
            .iter()
            .map(|&transform| BlockHashVariant {
                transform,
                hash: hash.clone(),
            })
            .collect()
    }

    #[test]
    fn empty_index_returns_no_matches() {
        let index = LibraryFingerprintIndex::new();
        assert_eq!(index.image_count, 0);
        assert!(index
            .recall(&variants(vec![0; 32]), 31)
            .unwrap()
            .matches
            .is_empty());
    }

    #[test]
    fn build_incremental_add_and_remove_are_consistent() {
        let first_id = Uuid::from_u128(1);
        let second_id = Uuid::from_u128(2);
        let first = row(first_id, vec![0; 32], 1);
        let second = row(second_id, vec![0xff; 32], 2);
        let mut index = LibraryFingerprintIndex::build(std::slice::from_ref(&first)).unwrap();
        assert_eq!(index.image_count, 1);
        assert_eq!(index.exact_file_matches(1, &[1; 32]), vec![first_id]);
        assert!(index.add(&second).unwrap());
        assert_eq!(index.image_count, 2);
        assert!(index.remove(first_id).unwrap());
        assert_eq!(index.image_count, 1);
        assert!(index.exact_pixel_matches(&[1; 32]).is_empty());
        assert_eq!(
            index.recall(&variants(vec![0xff; 32]), 0).unwrap().matches[0].image_id,
            second_id
        );
    }

    #[test]
    fn non_v2_rows_never_enter_the_index() {
        let mut legacy = row(Uuid::from_u128(1), vec![0; 32], 1);
        legacy.fingerprint_version = "1".to_string();
        let index = LibraryFingerprintIndex::build(&[legacy]).unwrap();
        assert_eq!(index.image_count, 0);
        assert!(index.block_tree.is_empty());
    }

    #[test]
    fn equal_distance_results_are_uuid_sorted_and_transform_deduplicated() {
        let mut rows = Vec::new();
        for value in [5u128, 1, 3, 2, 4] {
            rows.push(row(Uuid::from_u128(value), vec![0; 32], value as u8));
        }
        let index = LibraryFingerprintIndex::build(&rows).unwrap();
        assert_eq!(index.unique_hash_count(), 1);
        let result = index.recall(&variants(vec![0; 32]), 0).unwrap();
        assert_eq!(result.matches.len(), 5);
        assert_eq!(
            result
                .matches
                .iter()
                .map(|entry| entry.image_id)
                .collect::<Vec<_>>(),
            (1u128..=5).map(Uuid::from_u128).collect::<Vec<_>>()
        );
    }

    #[test]
    fn candidate_cap_is_stable() {
        let rows: Vec<_> = (0..300u128)
            .map(|value| row(Uuid::from_u128(value + 1), vec![0; 32], value as u8))
            .collect();
        let index = LibraryFingerprintIndex::build(&rows).unwrap();
        let result = index.recall(&variants(vec![0; 32]), 0).unwrap();
        assert!(result.truncated);
        assert_eq!(result.matches.len(), MAX_RECALL_CANDIDATES_PER_IMAGE);
        assert_eq!(result.matches[0].image_id, Uuid::from_u128(1));
        assert_eq!(result.matches[255].image_id, Uuid::from_u128(256));
    }

    #[test]
    fn malformed_v2_hash_invalidates_build() {
        let malformed = row(Uuid::from_u128(1), vec![0; 8], 1);
        assert!(LibraryFingerprintIndex::build(&[malformed]).is_err());
    }
}
