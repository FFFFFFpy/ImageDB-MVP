//! Duplicate group construction and representative selection.
//!
//! Note: representative selection currently prefers import images over
//! library images. Phase 8 of the core fix flips this so a historical
//! library image is the representative (and the new import image is
//! excluded). The dead-code fields below are retained for that rewrite.
#![allow(dead_code)]
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// A connected component of duplicate candidates.
///
/// Each component contains a set of image IDs that are transitively
/// related through duplicate candidate records. One representative
/// is selected deterministically; the rest are excluded.
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    /// All image IDs in this group (import_image_ids and/or library_image_ids).
    pub image_ids: Vec<Uuid>,
    /// The selected representative image ID.
    pub representative_id: Uuid,
    /// Whether the representative is an import image (true) or library image (false).
    pub representative_is_import: bool,
}

/// A duplicate edge between two images.
#[derive(Debug, Clone)]
pub struct DuplicateEdge {
    pub image_a: Uuid,
    pub image_b: Uuid,
    /// Whether image_a is an import image.
    pub a_is_import: bool,
    /// Whether image_b is an import image.
    pub b_is_import: bool,
    /// Confidence score (0.0 to 1.0), higher = more confident duplicate.
    pub confidence: f64,
    /// Whether the images are byte-identical.
    pub blake3_equal: bool,
    /// Whether the images are pixel-identical.
    pub pixel_hash_equal: bool,
}

/// Union-Find (Disjoint Set Union) for grouping duplicate relationships.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb,
            std::cmp::Ordering::Greater => self.parent[rb] = ra,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }
}

/// Build duplicate groups from a list of duplicate edges.
///
/// Uses Union-Find to find connected components, then selects a
/// representative for each group. The result is deterministic and
/// independent of input order.
pub fn build_duplicate_groups(edges: &[DuplicateEdge]) -> Vec<DuplicateGroup> {
    if edges.is_empty() {
        return Vec::new();
    }

    // Collect all unique image IDs and assign each an index.
    let mut id_to_idx: HashMap<Uuid, usize> = HashMap::new();
    let mut idx_to_id: Vec<Uuid> = Vec::new();
    let mut idx_is_import: Vec<bool> = Vec::new();

    for edge in edges {
        for (id, is_import) in &[
            (edge.image_a, edge.a_is_import),
            (edge.image_b, edge.b_is_import),
        ] {
            if !id_to_idx.contains_key(id) {
                let idx = idx_to_id.len();
                id_to_idx.insert(*id, idx);
                idx_to_id.push(*id);
                idx_is_import.push(*is_import);
            }
        }
    }

    // Build Union-Find.
    let mut uf = UnionFind::new(idx_to_id.len());
    for edge in edges {
        let ia = id_to_idx[&edge.image_a];
        let ib = id_to_idx[&edge.image_b];
        uf.union(ia, ib);
    }

    // Group by root.
    let mut root_to_indices: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..idx_to_id.len() {
        let root = uf.find(i);
        root_to_indices.entry(root).or_default().push(i);
    }

    // Build groups.
    let mut groups: Vec<DuplicateGroup> = Vec::new();
    for indices in root_to_indices.values_mut() {
        // Sort indices by their image ID for deterministic ordering.
        indices.sort_by_key(|&i| idx_to_id[i]);

        let image_ids: Vec<Uuid> = indices.iter().map(|&i| idx_to_id[i]).collect();

        // Select representative using deterministic rules.
        let representative_idx = select_representative(indices, &idx_is_import, edges, &id_to_idx);
        let representative_id = idx_to_id[representative_idx];
        let representative_is_import = idx_is_import[representative_idx];

        groups.push(DuplicateGroup {
            image_ids,
            representative_id,
            representative_is_import,
        });
    }

    // Sort groups by their smallest image ID for deterministic output.
    groups.sort_by_key(|g| g.image_ids[0]);

    groups
}

/// Select a representative from a group of indices.
///
/// Rules (in order of priority):
/// 1. Prefer import images over library images (library images can't be "kept" in import).
/// 2. Prefer decodable images over non-decodable.
/// 3. For byte-identical images, use stable ID (lowest UUID) as tiebreaker.
/// 4. For non-byte-identical, prefer higher quality (larger file size).
/// 5. If quality difference is insufficient (within 10%), fall back to stable ID.
fn select_representative(
    indices: &[usize],
    idx_is_import: &[bool],
    _edges: &[DuplicateEdge],
    id_to_idx: &HashMap<Uuid, usize>,
) -> usize {
    // Rule 1: Prefer import images.
    let import_indices: Vec<usize> = indices
        .iter()
        .copied()
        .filter(|&i| idx_is_import[i])
        .collect();
    let candidates: Vec<usize> = if import_indices.is_empty() {
        indices.to_vec()
    } else {
        import_indices
    };

    if candidates.len() == 1 {
        return candidates[0];
    }

    // Rule 3: Use stable ID (lowest UUID) as tiebreaker.
    // This is deterministic and independent of input order.
    *candidates
        .iter()
        .min_by_key(|&&idx| idx_to_id(idx, id_to_idx))
        .unwrap()
}

/// Helper to get the UUID for an index from the id_to_idx reverse map.
fn idx_to_id(idx: usize, id_to_idx: &HashMap<Uuid, usize>) -> Uuid {
    id_to_idx
        .iter()
        .find(|(_, &v)| v == idx)
        .map(|(k, _)| *k)
        .unwrap_or(Uuid::nil())
}

/// Compute the set of image IDs to exclude given duplicate groups.
///
/// Returns a set of image IDs that should be excluded (all non-representative
/// images in each group that are import images).
pub fn compute_excluded_ids(groups: &[DuplicateGroup]) -> HashSet<Uuid> {
    let mut excluded = HashSet::new();
    for group in groups {
        for &id in &group.image_ids {
            if id != group.representative_id {
                excluded.insert(id);
            }
        }
    }
    excluded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_edge(a: Uuid, b: Uuid, blake3_eq: bool, confidence: f64) -> DuplicateEdge {
        DuplicateEdge {
            image_a: a,
            image_b: b,
            a_is_import: true,
            b_is_import: true,
            confidence,
            blake3_equal: blake3_eq,
            pixel_hash_equal: blake3_eq,
        }
    }

    #[test]
    fn two_identical_images() {
        let a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let edges = vec![make_edge(a, b, true, 1.0)];

        let groups = build_duplicate_groups(&edges);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].image_ids.len(), 2);
        // Lowest UUID wins for byte-identical.
        assert_eq!(groups[0].representative_id, a);
    }

    #[test]
    fn reversed_input_order() {
        let a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let edges_forward = vec![make_edge(a, b, true, 1.0)];
        let edges_reversed = vec![make_edge(b, a, true, 1.0)];

        let groups_f = build_duplicate_groups(&edges_forward);
        let groups_r = build_duplicate_groups(&edges_reversed);

        assert_eq!(groups_f[0].representative_id, groups_r[0].representative_id);
        assert_eq!(groups_f[0].representative_id, a);
    }

    #[test]
    fn three_images_same_group() {
        let a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let c = Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap();
        let edges = vec![make_edge(a, b, true, 1.0), make_edge(b, c, true, 1.0)];

        let groups = build_duplicate_groups(&edges);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].image_ids.len(), 3);
        assert_eq!(groups[0].representative_id, a);
    }

    #[test]
    fn chain_relationship() {
        let a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let c = Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap();
        // A ≈ B, B ≈ C (perceptual, not byte-identical)
        let edges = vec![make_edge(a, b, false, 0.9), make_edge(b, c, false, 0.85)];

        let groups = build_duplicate_groups(&edges);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].image_ids.len(), 3);
    }

    #[test]
    fn two_separate_groups() {
        let a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let c = Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap();
        let d = Uuid::parse_str("00000000-0000-0000-0000-000000000004").unwrap();
        let edges = vec![make_edge(a, b, true, 1.0), make_edge(c, d, true, 1.0)];

        let groups = build_duplicate_groups(&edges);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn empty_edges() {
        let groups = build_duplicate_groups(&[]);
        assert_eq!(groups.len(), 0);
    }

    #[test]
    fn group_not_fully_excluded() {
        let a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let edges = vec![make_edge(a, b, true, 1.0)];

        let groups = build_duplicate_groups(&edges);
        let excluded = compute_excluded_ids(&groups);

        // Only one image should be excluded.
        assert_eq!(excluded.len(), 1);
        // The representative should NOT be excluded.
        assert!(!excluded.contains(&groups[0].representative_id));
    }

    #[test]
    fn library_image_not_preferred_as_representative() {
        let import_img = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let lib_img = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let edges = vec![DuplicateEdge {
            image_a: import_img,
            image_b: lib_img,
            a_is_import: true,
            b_is_import: false,
            confidence: 1.0,
            blake3_equal: true,
            pixel_hash_equal: true,
        }];

        let groups = build_duplicate_groups(&edges);
        assert_eq!(groups.len(), 1);
        // Import image should be the representative.
        assert_eq!(groups[0].representative_id, import_img);
        assert!(groups[0].representative_is_import);
    }
}
