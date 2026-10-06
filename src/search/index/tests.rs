#![allow(
    clippy::single_range_in_vec_init,
    reason = "Expected highlights are UTF-8 ranges, not scalar indices"
)]
use super::*;
use crate::{config::SearchMatch, utils::window_identity::WindowIdentity};

fn index(titles: &[String], config: &Config) -> SearchIndex {
    let mut bytes = MAX_INDEX_BYTES;
    let entries = titles
        .iter()
        .enumerate()
        .map(|(n, title)| {
            let entry = SearchEntry {
                identity: WindowIdentity::fixture(n + 1),
                key: crate::icon_cache::IconKey {
                    group: Arc::from("fixture.exe"),
                    identity: WindowIdentity::fixture(n + 1),
                },
                elevated: None,
                minimized: false,
                executable: Arc::from("程序.exe"),
                title: Arc::from(title.as_str()),
                app: Arc::from("微信"),
                highlights: RowHighlights::default(),
            };
            let fields = [SearchField::App, SearchField::Title, SearchField::Exe]
                .into_iter()
                .filter(|kind| config.search_fields.contains(*kind))
                .map(|kind| IndexedField {
                    kind,
                    normalized: matcher::normalize(entry.field(kind)),
                    phonetic: config
                        .search_pinyin
                        .then(|| PhoneticIndex::build(entry.field(kind), &mut bytes, &|| true))
                        .flatten(),
                })
                .collect();
            IndexedEntry { entry, fields }
        })
        .collect();
    SearchIndex {
        source: Arc::new(WindowSnapshot {
            groups: Default::default(),
            revision: 1,
        }),
        entries,
    }
}
fn title_config() -> Config {
    Config {
        search_fields: "title".parse().unwrap(),
        ..Default::default()
    }
}

#[test]
fn empty_query_is_ordered_bounded_and_cancelable() {
    let config = title_config();
    let index = index(
        &(0..210).map(|n| format!("窗口 {n}")).collect::<Vec<_>>(),
        &config,
    );
    let result = index.find("  ", &config, || true).unwrap();
    assert_eq!((result.total, result.entries.len()), (210, 50));
    assert_eq!(result.entries[49].identity.window, 50);
    assert!(index.find("窗口", &config, || false).is_none());
    assert!(index
        .find("no results", &config, || true)
        .unwrap()
        .entries
        .is_empty());
}

#[test]
fn maximum_window_set_retains_total_and_only_returns_the_configured_page() {
    let config = title_config();
    let titles = (0..crate::layout::MAX_WINDOWS)
        .map(|n| format!("报告{n}"))
        .collect::<Vec<_>>();
    let source = index(&titles, &config);
    let result = source.find("bg", &config, || true).unwrap();
    assert_eq!(result.total, crate::layout::MAX_WINDOWS);
    assert_eq!(result.entries.len(), config.search_max_results as usize);
}

#[test]
fn ranking_preserves_raw_order_and_switch_off_removes_phonetic_only_matches() {
    let mut config = title_config();
    let titles = ["w any x", "微信", "wxWidget", "doc wx"].map(str::to_owned);
    let result = index(&titles, &config)
        .find("wx", &config, || true)
        .unwrap();
    assert_eq!(
        result
            .entries
            .iter()
            .map(|e| e.identity.window)
            .collect::<Vec<_>>(),
        [3, 4, 2, 1]
    );
    assert_eq!(result.entries[2].highlights.primary, [0..6]);
    config.search_pinyin = false;
    let result = index(&titles, &config)
        .find("wx", &config, || true)
        .unwrap();
    assert_eq!(
        result
            .entries
            .iter()
            .map(|e| e.identity.window)
            .collect::<Vec<_>>(),
        [3, 4, 1]
    );
    let ties = index(&["微信".into(), "微信".into()], &title_config())
        .find("wx", &title_config(), || true)
        .unwrap();
    assert_eq!(ties.entries[0].identity.window, 1);
}

#[test]
fn displayed_ranges_follow_sanitized_text_fallback_and_enabled_fields() {
    let mut config = title_config();
    let text = "İ\u{85}微信😀".to_owned();
    let result = index(&[text], &config)
        .find("WEIXIN", &config, || true)
        .unwrap();
    let entry = &result.entries[0];
    let (_, display, _) = entry.display_text();
    assert_eq!(entry.highlights.query.as_ref(), "WEIXIN");
    assert_eq!(
        entry
            .highlights
            .primary
            .iter()
            .map(|r| &display[r.clone()])
            .collect::<Vec<_>>(),
        ["微信"]
    );
    config.search_fields = "app".parse().unwrap();
    let result = index(&["Unrelated title".into()], &config)
        .find("wx", &config, || true)
        .unwrap();
    assert!(result.entries[0].highlights.primary.is_empty());
    let result = index(&["".into()], &config)
        .find("wx", &config, || true)
        .unwrap();
    assert_eq!(result.entries[0].highlights.primary, [0..6]);
    config.search_fields = "exe".parse().unwrap();
    let result = index(&["Unrelated title".into()], &config)
        .find("cx", &config, || true)
        .unwrap();
    assert_eq!(result.entries[0].highlights.secondary, [0..6]);
    config.search_fields = "title".parse().unwrap();
    assert!(index(&["Unrelated title".into()], &config)
        .find("cx", &config, || true)
        .unwrap()
        .entries
        .is_empty());
}

#[test]
fn oversized_phonetic_fields_retain_original_search_and_raw_mode_semantics() {
    let mut config = title_config();
    let long = "微信".repeat(300);
    let source = index(&[long], &config);
    assert!(source.entries[0].fields[0].phonetic.is_none());
    assert_eq!(source.find("微信", &config, || true).unwrap().total, 1);
    assert_eq!(source.find("weixin", &config, || true).unwrap().total, 0);
    for (mode, expected) in [
        (SearchMatch::Prefix, 0),
        (SearchMatch::Contains, 0),
        (SearchMatch::Fuzzy, 1),
    ] {
        config.search_match = mode;
        assert_eq!(
            index(&["微我的信".into()], &config)
                .find("wx", &config, || true)
                .unwrap()
                .total,
            expected
        );
    }
}
