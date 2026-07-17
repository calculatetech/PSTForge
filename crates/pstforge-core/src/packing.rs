use std::collections::BTreeSet;

use thiserror::Error;

use crate::ItemKey;

/// Durable candidate metadata needed to assign mail to output parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackCandidate {
    pub key: ItemKey,
    pub folder_path: Vec<String>,
    pub payload_bytes: u64,
}

/// A deterministic output-part assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartAssignment {
    pub index: u32,
    pub candidates: Vec<PackCandidate>,
    pub estimated_bytes: u64,
    pub oversize: bool,
}

/// Computes the complete serialized size of a proposed part before its safety reserve.
///
/// Implementations include mandatory folders, replicated source folders, tables,
/// B-trees, allocation maps, and payload blocks. The packer deliberately asks about
/// the whole part so nonlinear table and page growth cannot be hidden in a per-item
/// approximation.
pub trait PartSizeEstimator {
    fn estimate_part_bytes(&self, candidates: &[PackCandidate]) -> Result<u64, PackingError>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PackingError {
    #[error("maximum PST size must be greater than zero")]
    ZeroMaximumSize,
    #[error("part safety reserve {reserve} exceeds maximum PST size {maximum}")]
    ReserveExceedsMaximum { reserve: u64, maximum: u64 },
    #[error("candidate folder path must contain only non-empty components")]
    InvalidFolderPath,
    #[error("duplicate candidate key in packing input: {0:?}")]
    DuplicateCandidate(ItemKey),
    #[error("part count exceeds the supported index range")]
    TooManyParts,
    #[error("size calculation overflow")]
    SizeOverflow,
    #[error("size estimator rejected the proposed part: {0}")]
    Estimator(String),
}

/// Sort candidates canonically and pack them without ever splitting one item.
///
/// A singleton larger than the effective target is emitted as an explicitly
/// oversize part. Every other part is guaranteed to have an estimate plus reserve
/// no greater than `maximum_bytes`.
pub fn pack_candidates(
    mut candidates: Vec<PackCandidate>,
    maximum_bytes: u64,
    safety_reserve_bytes: u64,
    estimator: &impl PartSizeEstimator,
) -> Result<Vec<PartAssignment>, PackingError> {
    if maximum_bytes == 0 {
        return Err(PackingError::ZeroMaximumSize);
    }
    let effective_limit = maximum_bytes.checked_sub(safety_reserve_bytes).ok_or(
        PackingError::ReserveExceedsMaximum {
            reserve: safety_reserve_bytes,
            maximum: maximum_bytes,
        },
    )?;
    for candidate in &candidates {
        if candidate.folder_path.iter().any(String::is_empty) {
            return Err(PackingError::InvalidFolderPath);
        }
    }
    candidates.sort_by(|left, right| {
        left.folder_path
            .cmp(&right.folder_path)
            .then_with(|| left.key.cmp(&right.key))
    });
    let mut keys = BTreeSet::new();
    for candidate in &candidates {
        if !keys.insert(candidate.key) {
            return Err(PackingError::DuplicateCandidate(candidate.key));
        }
    }

    let mut parts = Vec::new();
    let mut current = Vec::new();
    let mut current_estimate = 0;
    for candidate in candidates {
        let mut proposed = current.clone();
        proposed.push(candidate.clone());
        let proposed_estimate = estimator.estimate_part_bytes(&proposed)?;
        if !current.is_empty() && proposed_estimate > effective_limit {
            push_part(&mut parts, current, current_estimate, false)?;
            current = vec![candidate];
            current_estimate = estimator.estimate_part_bytes(&current)?;
            if current_estimate > effective_limit {
                push_part(&mut parts, current, current_estimate, true)?;
                current = Vec::new();
                current_estimate = 0;
            }
        } else if proposed_estimate > effective_limit {
            push_part(&mut parts, proposed, proposed_estimate, true)?;
            current = Vec::new();
            current_estimate = 0;
        } else {
            current = proposed;
            current_estimate = proposed_estimate;
        }
    }
    if !current.is_empty() {
        push_part(&mut parts, current, current_estimate, false)?;
    }
    Ok(parts)
}

fn push_part(
    parts: &mut Vec<PartAssignment>,
    candidates: Vec<PackCandidate>,
    estimated_bytes: u64,
    oversize: bool,
) -> Result<(), PackingError> {
    let index = u32::try_from(parts.len())
        .ok()
        .and_then(|index| index.checked_add(1))
        .ok_or(PackingError::TooManyParts)?;
    parts.push(PartAssignment {
        index,
        candidates,
        estimated_bytes,
        oversize,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RecoveryProvenance;

    struct LayoutEstimator;

    impl PartSizeEstimator for LayoutEstimator {
        fn estimate_part_bytes(&self, candidates: &[PackCandidate]) -> Result<u64, PackingError> {
            let folders = candidates
                .iter()
                .flat_map(|candidate| {
                    (1..=candidate.folder_path.len())
                        .map(|length| candidate.folder_path[..length].to_vec())
                })
                .collect::<BTreeSet<_>>();
            let payload = candidates.iter().try_fold(0_u64, |total, candidate| {
                total
                    .checked_add(candidate.payload_bytes)
                    .ok_or(PackingError::SizeOverflow)
            })?;
            let table_pages = candidates.len().div_ceil(2) as u64;
            100_u64
                .checked_add(payload)
                .and_then(|value| value.checked_add(folders.len() as u64 * 10))
                .and_then(|value| value.checked_add(table_pages * 20))
                .ok_or(PackingError::SizeOverflow)
        }
    }

    fn candidate(path: &[&str], node: u32, bytes: u64) -> PackCandidate {
        PackCandidate {
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(node),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: path.iter().map(|value| (*value).to_owned()).collect(),
            payload_bytes: bytes,
        }
    }

    #[test]
    fn exact_boundary_stays_in_one_part_and_one_byte_over_splits() {
        let exact = pack_candidates(
            vec![candidate(&["Inbox"], 1, 25), candidate(&["Inbox"], 2, 25)],
            180,
            0,
            &LayoutEstimator,
        )
        .unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].estimated_bytes, 180);

        let over = pack_candidates(
            vec![candidate(&["Inbox"], 1, 25), candidate(&["Inbox"], 2, 26)],
            180,
            0,
            &LayoutEstimator,
        )
        .unwrap();
        assert_eq!(over.len(), 2);
        assert!(over.iter().all(|part| !part.oversize));
    }

    #[test]
    fn nonlinear_table_and_folder_growth_are_part_of_every_trial() {
        let parts = pack_candidates(
            vec![
                candidate(&["A"], 1, 1),
                candidate(&["A"], 2, 1),
                candidate(&["A", "B"], 3, 1),
            ],
            150,
            0,
            &LayoutEstimator,
        )
        .unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].candidates.len(), 2);
        assert_eq!(parts[1].candidates[0].folder_path, ["A", "B"]);
    }

    #[test]
    fn canonical_order_is_independent_of_discovery_order() {
        let input = vec![
            candidate(&["Z"], 9, 1),
            candidate(&["A"], 5, 1),
            candidate(&["A"], 2, 1),
        ];
        let mut reversed = input.clone();
        reversed.reverse();
        let first = pack_candidates(input, 1_000, 50, &LayoutEstimator).unwrap();
        let second = pack_candidates(reversed, 1_000, 50, &LayoutEstimator).unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].candidates[0].key.source_node_id, Some(2));
    }

    #[test]
    fn indivisible_oversize_candidate_is_alone_and_accounted_once() {
        let parts = pack_candidates(
            vec![candidate(&["A"], 1, 500), candidate(&["A"], 2, 1)],
            200,
            10,
            &LayoutEstimator,
        )
        .unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].oversize);
        assert_eq!(parts[0].candidates.len(), 1);
        assert!(!parts[1].oversize);
        assert_eq!(
            parts
                .iter()
                .map(|part| part.candidates.len())
                .sum::<usize>(),
            2
        );
    }

    #[test]
    fn duplicate_keys_are_rejected_before_assignment() {
        let duplicate = candidate(&["A"], 1, 1);
        assert!(matches!(
            pack_candidates(
                vec![duplicate.clone(), duplicate],
                1_000,
                0,
                &LayoutEstimator
            ),
            Err(PackingError::DuplicateCandidate(_))
        ));
    }
}
