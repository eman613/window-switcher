//! Bounded Unicode matching. Scores never depend on window titles outside memory.
use crate::config::SearchMatch;

pub(super) fn normalize(value: &str) -> String {
    value.chars().flat_map(char::to_lowercase).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Score(u8, usize, usize);

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
            None => return Some(Score(2, offset + 1 - start - matched, start)),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
