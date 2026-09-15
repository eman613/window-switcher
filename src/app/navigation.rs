use indexmap::IndexSet;
use std::hash::Hash;

pub(super) fn cycle_index(current: usize, count: usize, reverse: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let current = current.min(count - 1);
    Some(if reverse {
        if current == 0 {
            count - 1
        } else {
            current - 1
        }
    } else if current + 1 == count {
        0
    } else {
        current + 1
    })
}

pub(super) fn reconcile_cycle<T: Copy + Eq + Hash>(
    previous: &[T],
    cached_index: usize,
    fresh: &[T],
    reverse: bool,
) -> Option<(Vec<T>, usize)> {
    let mut remaining: IndexSet<T> = fresh.iter().copied().collect();
    let mut ordered: Vec<T> = previous
        .iter()
        .copied()
        .filter(|value| remaining.swap_remove(value))
        .collect();
    ordered.extend(remaining);
    let base = previous
        .get(cached_index)
        .and_then(|selected| ordered.iter().position(|value| value == selected))
        .unwrap_or_else(|| cached_index.min(ordered.len().saturating_sub(1)));
    let index = cycle_index(base, ordered.len(), reverse)?;
    Some((ordered, index))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reverse_after_six_windows_shrink_to_three_is_in_bounds() {
        let (windows, index) = reconcile_cycle(&[0, 1, 2, 3, 4, 5], 5, &[0, 1, 2], true).unwrap();
        assert!(index < windows.len());
        assert_eq!(windows[index], 1);
    }
    #[test]
    fn selected_identity_is_remapped_and_empty_snapshots_cancel() {
        let (windows, index) = reconcile_cycle(&[10, 20, 30, 40], 2, &[30, 40, 50], true).unwrap();
        assert_eq!(windows[index], 50);
        assert!(reconcile_cycle(&[1, 2], 1, &[], false).is_none());
        for count in 1..=6 {
            for current in [0, 1, 5, usize::MAX] {
                for reverse in [false, true] {
                    assert!(cycle_index(current, count, reverse).unwrap() < count);
                }
            }
        }
    }
}
