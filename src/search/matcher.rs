//! Bounded Unicode matching. Scores never depend on window titles outside memory.
use crate::config::SearchMatch;

pub(super) fn normalize(value: &str) -> String {
    value.chars().flat_map(char::to_lowercase).collect()
}

pub(crate) fn highlight_ranges(
    value: &str,
    query: &str,
    mode: SearchMatch,
) -> Vec<std::ops::Range<usize>> {
    let query: Vec<_> = normalize(query.trim()).chars().collect();
    if query.is_empty() {
        return Vec::new();
    }
    let normalized: Vec<_> = value
        .char_indices()
        .flat_map(|(start, character)| {
            character
                .to_lowercase()
                .map(move |folded| (folded, start, start + character.len_utf8()))
        })
        .collect();
    let contiguous = normalized
        .windows(query.len())
        .position(|window| window.iter().map(|entry| entry.0).eq(query.iter().copied()));
    let indices: Vec<usize> = if let Some(start) =
        contiguous.filter(|start| mode != SearchMatch::Prefix || *start == 0)
    {
        (start..start + query.len()).collect()
    } else if mode == SearchMatch::Fuzzy {
        let mut wanted = 0;
        let mut indices = Vec::new();
        for (index, entry) in normalized.iter().enumerate() {
            if entry.0 == query[wanted] {
                indices.push(index);
                wanted += 1;
                if wanted == query.len() {
                    break;
                }
            }
        }
        if wanted != query.len() {
            return Vec::new();
        }
        indices
    } else {
        return Vec::new();
    };
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for index in indices {
        let (_, start, end) = normalized[index];
        if let Some(last) = ranges.last_mut().filter(|last| last.end >= start) {
            last.end = end;
        } else {
            ranges.push(start..end);
        }
    }
    ranges
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Score(pub(super) u8, pub(super) usize, pub(super) usize);

pub(super) fn score(field: &str, query: &str, mode: SearchMatch) -> Option<Score> {
    if query.is_empty() {
        return Some(Score(0, 0, 0));
    }
    if field.starts_with(query) {
        return Some(Score(0, 0, field.chars().count()));
    }
    if mode == SearchMatch::Prefix {
        return None;
    }
    if let Some(start) = field.find(query) {
        return Some(Score(
            1,
            field[..start].chars().count(),
            field.chars().count(),
        ));
    }
    if mode == SearchMatch::Contains {
        return None;
    }
    let mut wanted = query.chars();
    let mut next = wanted.next()?;
    let (mut first, mut matched) = (None, 0);
    for (offset, character) in field.chars().enumerate() {
        if character != next {
            continue;
        }
        let start = *first.get_or_insert(offset);
        matched += 1;
        match wanted.next() {
            Some(character) => next = character,
            None => return Some(Score(4, offset + 1 - start - matched, start)),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlighted_ranges_preserve_original_unicode_boundaries_and_match_mode() {
        let value = "İ 工程 🪟 report";
        let ranges = highlight_ranges(value, "i\u{307}🪟", SearchMatch::Fuzzy);
        assert_eq!(
            ranges
                .iter()
                .map(|range| &value[range.clone()])
                .collect::<Vec<_>>(),
            ["İ", "🪟"]
        );
        assert!(highlight_ranges(value, "工🪟", SearchMatch::Contains).is_empty());
        assert!(highlight_ranges(value, "工程", SearchMatch::Prefix).is_empty());
        assert!(highlight_ranges(value, "missing", SearchMatch::Fuzzy).is_empty());
    }

    #[test]
    fn unicode_and_three_modes_have_distinct_semantics() {
        let field = normalize("工程报告 — Éditeur 🪟");
        assert!(score(&field, "工程", SearchMatch::Prefix).is_some());
        assert!(score(&field, "报告", SearchMatch::Prefix).is_none());
        assert!(score(&field, &normalize("ÉDIT"), SearchMatch::Contains).is_some());
        assert!(score(&field, "工报", SearchMatch::Contains).is_none());
        assert!(score(&field, "工报", SearchMatch::Fuzzy).is_some());
        assert!(score(&field, "报工", SearchMatch::Fuzzy).is_none());
        assert!(score(&field, "🪟", SearchMatch::Contains).is_some());
        assert_eq!(
            score("", "", SearchMatch::Prefix),
            score(&field, "", SearchMatch::Fuzzy)
        );
    }

    #[test]
    fn contiguous_matches_rank_before_sparse_matches() {
        let query = "abc";
        let prefix = score("abc title", query, SearchMatch::Fuzzy).unwrap();
        let contains = score("title abc", query, SearchMatch::Fuzzy).unwrap();
        let sparse = score("a long b long c", query, SearchMatch::Fuzzy).unwrap();
        assert!(prefix < contains && contains < sparse);
        assert!(score("ab", query, SearchMatch::Fuzzy).is_none());
        assert!(score("a", "aa", SearchMatch::Fuzzy).is_none());
    }
}
