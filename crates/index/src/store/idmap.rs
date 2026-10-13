//! Maps SQLite rowids to dense `0..n` indices (the ids used inside an `IndexSnapshot`).

use std::collections::HashMap;

/// Rowid → dense index. Dense (a flat `Vec`) when the rowids are packed, a hash map otherwise.
#[derive(Debug)]
pub(crate) enum IdMap {
    Dense(Vec<u32>),
    Sparse(HashMap<i64, u32>),
}

const ABSENT: u32 = u32::MAX;

impl IdMap {
    /// A map for `count` ids whose largest value is `max_id`.
    pub(crate) fn new(max_id: i64, count: usize) -> Self {
        let max = usize::try_from(max_id).unwrap_or(0);
        if max <= count.saturating_mul(8).saturating_add(4096) {
            Self::Dense(vec![ABSENT; max + 1])
        } else {
            Self::Sparse(HashMap::with_capacity(count))
        }
    }

    pub(crate) fn insert(&mut self, id: i64, index: u32) {
        match self {
            Self::Dense(v) => {
                if let Some(slot) = usize::try_from(id).ok().and_then(|i| v.get_mut(i)) {
                    *slot = index;
                }
            }
            Self::Sparse(m) => {
                m.insert(id, index);
            }
        }
    }

    pub(crate) fn get(&self, id: i64) -> Option<u32> {
        match self {
            Self::Dense(v) => usize::try_from(id)
                .ok()
                .and_then(|i| v.get(i))
                .copied()
                .filter(|&i| i != ABSENT),
            Self::Sparse(m) => m.get(&id).copied(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_and_sparse_behave_the_same() {
        for (max, count) in [(10, 5), (1_000_000_000, 3)] {
            let mut m = IdMap::new(max, count);
            assert!(matches!(
                (&m, max > 100),
                (IdMap::Dense(_), false) | (IdMap::Sparse(_), true)
            ));
            m.insert(3, 0);
            m.insert(max, 1);
            assert_eq!(m.get(3), Some(0));
            assert_eq!(m.get(max), Some(1));
            assert_eq!(m.get(4), None);
            assert_eq!(m.get(-1), None);
        }
    }
}
