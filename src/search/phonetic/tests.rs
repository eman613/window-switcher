#![allow(
    clippy::single_range_in_vec_init,
    reason = "Expected highlights are UTF-8 ranges, not scalar indices"
)]
use super::*;
fn hit(text: &str, query: &str, mode: SearchMatch) -> Option<Match> {
    let mut work = MAX_QUERY_WORK;
    let index = PhoneticIndex::build(text, &mut (1024 * 1024), &|| true)?;
    index.find(
        &super::super::matcher::normalize(query)
            .chars()
            .map(fold)
            .collect::<Vec<_>>(),
        mode,
        &mut work,
        &|| true,
    )
}

#[test]
fn full_initials_polyphones_and_mixed_originals_match_without_spelling_correction() {
    for (text, queries) in [
        (
            "微信",
            vec!["weixin", "wx", "wei信", "微xin", "weix", "WEIXIN"],
        ),
        (
            "重庆银行",
            vec!["chongqingyinhang", "cqyh", "zhongqingyinxing"],
        ),
        ("音乐播放器", vec!["yinyuebofangqi", "yybfq"]),
        ("长安汽车", vec!["changanqiche", "caqc"]),
        ("厦门", vec!["xiamen", "xm"]),
        ("重新启动", vec!["chongxinqidong", "cxqd"]),
        (
            "微信PDF2026",
            vec!["weixinpdf2026", "wxPDF2026", "wei信pdf2026"],
        ),
        ("绿色", vec!["lvse", "lüse", "ls"]),
        ("繁體銀行", vec!["fantiyinhang", "ftyh"]),
    ] {
        for query in queries {
            assert!(
                hit(text, query, SearchMatch::Contains).is_some(),
                "{text}: {query}"
            );
        }
    }
    for query in ["wexin", "weixni", "xinwei", "xx", "微", "微信"] {
        assert!(hit("微信", query, SearchMatch::Fuzzy).is_none(), "{query}");
    }
    assert!(hit("Visual Studio", "vs", SearchMatch::Fuzzy).is_none());
    assert!(hit("微信", "wixin", SearchMatch::Fuzzy).is_none());
}

#[test]
fn modes_respect_character_boundaries_and_scores_distinguish_channels() {
    assert!(hit("我的微信", "wx", SearchMatch::Prefix).is_none());
    assert!(hit("我的微信", "wx", SearchMatch::Contains).is_some());
    assert!(hit("微我的信", "weixin", SearchMatch::Contains).is_none());
    let sparse = hit("微我的信", "weixin", SearchMatch::Fuzzy).unwrap();
    assert_eq!(sparse.score.0, 5);
    assert_eq!(sparse.ranges, [0..3, 9..12]);
    assert!(hit("微信", "eixin", SearchMatch::Contains).is_none());
    assert_eq!(
        hit("微信", "weixin", SearchMatch::Prefix).unwrap().score.0,
        2
    );
    assert_eq!(hit("微信", "wx", SearchMatch::Prefix).unwrap().score.0, 3);
}

#[test]
fn ranges_refer_to_original_utf8_even_with_case_expansion_and_unknown_characters() {
    for text in ["İ微信😀", "e\u{301}微信", "𠮷微信"] {
        let matched = hit(text, "weixin", SearchMatch::Contains).unwrap();
        assert_eq!(
            matched
                .ranges
                .iter()
                .map(|r| &text[r.clone()])
                .collect::<Vec<_>>(),
            ["微信"]
        );
    }
    assert_eq!(
        hit("𠮷微信", "𠮷weixin", SearchMatch::Prefix)
            .unwrap()
            .ranges,
        [0..10]
    );
    assert_eq!(
        hit("İ微信", "i\u{307}weixin", SearchMatch::Prefix)
            .unwrap()
            .ranges,
        [0..8]
    );
}

#[test]
fn storage_work_and_cancellation_are_bounded_without_enumerating_polyphonic_products() {
    let mut storage = usize::MAX;
    let mut bytes = 1;
    assert!(PhoneticIndex::build("重庆", &mut bytes, &|| true).is_none());
    assert_eq!(bytes, 1);
    assert!(PhoneticIndex::build(
        &"重".repeat(MAX_FIELD_CHARACTERS + 1),
        &mut storage,
        &|| true
    )
    .is_none());
    let index = PhoneticIndex::build(&"重行".repeat(200), &mut (1024 * 1024), &|| true).unwrap();
    assert_eq!(index.tokens.len(), 400);
    let query = vec!['c'; 128];
    let mut work = 100;
    assert!(index
        .find(&query, SearchMatch::Fuzzy, &mut work, &|| true)
        .is_none());
    assert_eq!(work, 0);
    let mut work = MAX_QUERY_WORK;
    assert!(index
        .find(&['c'], SearchMatch::Fuzzy, &mut work, &|| false)
        .is_none());
    assert!(PhoneticIndex::build("微信", &mut storage, &|| false).is_none());
    let calls = std::cell::Cell::new(0);
    assert!(index
        .find(&query, SearchMatch::Fuzzy, &mut work, &|| {
            calls.set(calls.get() + 1);
            calls.get() < 3
        })
        .is_none());
    assert_eq!(calls.get(), 3);
}
