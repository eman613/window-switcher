//! Optional, bounded character readings. No title or query survives its snapshot.
mod matching;
#[cfg(test)]
mod tests;

use super::matcher::Score;
use crate::config::SearchMatch;
use pinyin::ToPinyinMulti;
use std::ops::Range;

const MAX_FIELD_CHARACTERS: usize = 512;
const MAX_READINGS: usize = 16;
const MAX_FIELD_WORK: usize = 65_536;
pub(super) const MAX_QUERY_WORK: usize = 4_000_000;

pub(super) fn fold(c: char) -> char {
    if c == 'ü' {
        'v'
    } else {
        c
    }
}

struct Token {
    original: Range<usize>,
    character: char,
    readings: Vec<&'static str>,
}

pub(super) struct PhoneticIndex {
    tokens: Vec<Token>,
}

pub(super) struct Match {
    pub score: Score,
    pub ranges: Vec<Range<usize>>,
}

impl PhoneticIndex {
    pub(super) fn build(
        value: &str,
        available: &mut usize,
        current: &dyn Fn() -> bool,
    ) -> Option<Self> {
        if value.is_ascii() {
            return None;
        }
        let count = value.chars().take(MAX_FIELD_CHARACTERS + 1).count();
        if count > MAX_FIELD_CHARACTERS || count * std::mem::size_of::<Token>() > *available {
            return None;
        }
        let mut tokens = Vec::with_capacity(count);
        let mut bytes = tokens.capacity() * std::mem::size_of::<Token>();
        let mut has_readings = false;
        for (start, character) in value.char_indices() {
            if !current() {
                return None;
            }
            let mut readings = Vec::new();
            if let Some(multi) = character.to_pinyin_multi() {
                for reading in multi {
                    let plain = reading.plain();
                    if !readings.contains(&plain) {
                        readings.push(plain);
                    }
                    if readings.len() > MAX_READINGS {
                        return None;
                    }
                }
            }
            bytes = bytes.checked_add(readings.capacity() * std::mem::size_of::<&str>())?;
            if bytes > *available {
                return None;
            }
            has_readings |= !readings.is_empty();
            tokens.push(Token {
                original: start..start + character.len_utf8(),
                character,
                readings,
            });
        }
        if !has_readings {
            return None;
        }
        *available -= bytes;
        Some(Self { tokens })
    }

    pub(super) fn find(
        &self,
        query: &[char],
        mode: SearchMatch,
        remaining: &mut usize,
        current: &dyn Fn() -> bool,
    ) -> Option<Match> {
        if query.is_empty() || !query.iter().any(char::is_ascii_alphabetic) {
            return None;
        }
        let mut work = matching::Work::new(remaining, current);
        let full = matching::run(self, query, mode, false, &mut work)?;
        if full.as_ref().is_some_and(|m| m.score.0 == 2) {
            return full;
        }
        let initials = matching::run(self, query, mode, true, &mut work)?;
        full.into_iter().chain(initials).min_by_key(|m| m.score)
    }
}
