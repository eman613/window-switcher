use super::{
    matcher::{self, Score},
    phonetic::{fold, PhoneticIndex},
};
use crate::config::{SearchField, SearchMatch};
use std::ops::Range;

pub(super) struct IndexedField {
    pub kind: SearchField,
    pub normalized: String,
    pub phonetic: Option<PhoneticIndex>,
}
pub(super) struct Query {
    raw: String,
    phonetic: Vec<char>,
    mode: SearchMatch,
}
impl Query {
    pub fn new(value: &str, mode: SearchMatch) -> Self {
        let raw = matcher::normalize(value.trim());
        let phonetic = raw.chars().map(fold).collect();
        Self {
            raw,
            phonetic,
            mode,
        }
    }
    pub fn score(
        &self,
        field: &IndexedField,
        work: &mut usize,
        current: &dyn Fn() -> bool,
    ) -> Option<Score> {
        let raw = matcher::score(&field.normalized, &self.raw, self.mode);
        if raw.is_some_and(|score| score.0 <= 1) {
            return raw;
        }
        let phonetic = field
            .phonetic
            .as_ref()
            .and_then(|index| index.find(&self.phonetic, self.mode, work, current))
            .map(|hit| hit.score);
        raw.into_iter().chain(phonetic).min()
    }
    pub fn ranges(
        &self,
        display: &str,
        pinyin: bool,
        work: &mut usize,
        current: &dyn Fn() -> bool,
    ) -> Vec<Range<usize>> {
        let raw = matcher::score(&matcher::normalize(display), &self.raw, self.mode);
        if !pinyin || raw.is_some_and(|score| score.0 <= 1) {
            return matcher::highlight_ranges(display, &self.raw, self.mode);
        }
        // One transient display index at a time; never retained by the UI.
        let phonetic = PhoneticIndex::build(display, &mut (64 * 1024), current)
            .and_then(|index| index.find(&self.phonetic, self.mode, work, current));
        if let Some(hit) = phonetic.filter(|hit| raw.is_none_or(|raw| hit.score < raw)) {
            hit.ranges
        } else {
            matcher::highlight_ranges(display, &self.raw, self.mode)
        }
    }
}
