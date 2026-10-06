use super::{fold, Match, PhoneticIndex, Score, SearchMatch, MAX_FIELD_WORK};

pub(super) struct Work<'a> {
    remaining: &'a mut usize,
    field: usize,
    current: &'a dyn Fn() -> bool,
}
impl<'a> Work<'a> {
    pub(super) fn new(remaining: &'a mut usize, current: &'a dyn Fn() -> bool) -> Self {
        Self {
            remaining,
            field: MAX_FIELD_WORK,
            current,
        }
    }
    fn step(&mut self) -> Option<()> {
        if self.field == 0
            || *self.remaining == 0
            || (self.field.is_multiple_of(64) && !(self.current)())
        {
            return None;
        }
        self.field -= 1;
        *self.remaining -= 1;
        Some(())
    }
}

#[derive(Clone, Copy)]
struct State {
    start: usize,
    skips: usize,
    trace: Option<usize>,
}
struct Trace {
    token: usize,
    previous: Option<usize>,
}

fn retain(slot: &mut Option<State>, candidate: State) -> bool {
    if slot.is_none_or(|old| (candidate.skips, candidate.start) < (old.skips, old.start)) {
        *slot = Some(candidate);
        true
    } else {
        false
    }
}

// Only a final syllable may be incomplete. Literal Unicode folds must be complete.
fn consume(
    chars: impl Iterator<Item = char>,
    query: &[char],
    partial: bool,
    work: &mut Work<'_>,
) -> Option<usize> {
    let mut count = 0;
    for c in chars {
        work.step()?;
        if count == query.len() {
            return Some(if partial { count } else { 0 });
        }
        if fold(c) != query[count] {
            return Some(0);
        }
        count += 1;
    }
    Some(count)
}

// Outer None is cancellation/budget exhaustion; inner None is an ordinary miss.
pub(super) fn run(
    index: &PhoneticIndex,
    query: &[char],
    mode: SearchMatch,
    initials: bool,
    work: &mut Work<'_>,
) -> Option<Option<Match>> {
    let mut states = vec![None; (query.len() + 1) * 2];
    let mut traces: Vec<Trace> = Vec::new();
    let mut best: Option<(Score, usize)> = None;
    for (position, token) in index.tokens.iter().enumerate() {
        work.step()?;
        if mode != SearchMatch::Prefix || position == 0 {
            states[0] = Some(State {
                start: position,
                skips: 0,
                trace: None,
            });
        }
        let mut next = vec![None; states.len()];
        for (slot, state) in states.iter().enumerate() {
            work.step()?;
            let Some(state) = *state else {
                continue;
            };
            let offset = slot / 2;
            let used = slot % 2 != 0;
            if offset > 0 && mode == SearchMatch::Fuzzy {
                retain(
                    &mut next[slot],
                    State {
                        skips: state.skips + 1,
                        ..state
                    },
                );
            }
            let literal = consume(
                token.character.to_lowercase(),
                &query[offset..],
                false,
                work,
            )?;
            // Readings are small static strings, not a cartesian product of titles.
            for variant in 0..=token.readings.len() {
                work.step()?;
                let (count, used) = if variant == 0 {
                    (literal, used)
                } else {
                    let reading = token.readings[variant - 1];
                    let count = if initials {
                        consume(reading.chars().take(1), &query[offset..], false, work)?
                    } else {
                        consume(reading.chars(), &query[offset..], true, work)?
                    };
                    (count, true)
                };
                if count == 0 {
                    continue;
                }
                let end = offset + count;
                let trace = traces.len();
                let candidate = State {
                    trace: Some(trace),
                    ..state
                };
                let accepted = if end == query.len() {
                    let score = if state.skips == 0 {
                        Score(
                            if initials { 3 } else { 2 },
                            state.start,
                            index.tokens.len(),
                        )
                    } else {
                        Score(if initials { 6 } else { 5 }, state.skips, state.start)
                    };
                    if used && best.as_ref().is_none_or(|(old, _)| score < *old) {
                        best = Some((score, trace));
                        true
                    } else {
                        false
                    }
                } else {
                    retain(&mut next[end * 2 + usize::from(used)], candidate)
                };
                if accepted {
                    traces.push(Trace {
                        token: position,
                        previous: state.trace,
                    });
                }
            }
        }
        states = next;
    }
    Some(best.map(|(score, last)| {
        let mut path = Vec::new();
        let mut trace = Some(last);
        while let Some(n) = trace {
            path.push(index.tokens[traces[n].token].original.clone());
            trace = traces[n].previous;
        }
        path.reverse();
        let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
        for range in path {
            if let Some(previous) = ranges.last_mut().filter(|p| p.end == range.start) {
                previous.end = range.end;
            } else {
                ranges.push(range);
            }
        }
        Match { score, ranges }
    }))
}
