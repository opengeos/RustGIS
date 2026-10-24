use super::{Cache, MAX_TEXT_BYTES, SHARD_BYTES, Widths};
use crate::text::{font, measure, shape};

/// Frozen pre-cache measurement: independent of `measure_uncached`, so changes to the miss path
/// cannot make the expected and actual results drift together. Font/shaping behavior stays shared.
fn original_measure(text: &str, size: f32, bold: bool) -> f32 {
    let Some(f) = font(bold) else {
        return text.chars().count() as f32 * size * 0.6;
    };
    shape(text, size * 1.33, f, None).1
}

/// Compares cold and warm widths bit for bit across scripts, missing glyphs, kerning and sizes.
/// Extreme inputs keep the original NaN/infinity results instead of being clamped by the cache.
#[test]
fn measured_widths_match_the_original_bits() {
    let long = "Long label 中 ".repeat(500);
    let at_limit = "x".repeat(MAX_TEXT_BYTES);
    for text in ["", "AV WA", "Mixed 中英 日本語 한글", "Ελλάδα Беларусь العربية", "a\u{301}\u{200d}", "\u{10FFFD}", &long, &at_limit]
    {
        for size in
            [0.0, -0.0, -8.0, f32::MIN_POSITIVE, f32::from_bits(1), 4.0, 10.5, 72.0, f32::MAX, f32::INFINITY, f32::NEG_INFINITY, f32::NAN]
        {
            for bold in [false, true] {
                let expected = original_measure(text, size, bold).to_bits();
                for _ in 0..2 {
                    assert_eq!(measure(text, size, bold).to_bits(), expected, "{text:?}, {size:?}, bold={bold}");
                }
            }
        }
    }
    // The cache must preserve shaping's 500-character cutoff, not remeasure a shortened key.
    assert_eq!(measure(&"A".repeat(500), 10.0, false).to_bits(), measure(&"A".repeat(501), 10.0, false).to_bits());
}

/// A warm key skips measurement, while text, exact size bits and weight select separate entries.
/// The no-font estimate is just another callback result and retains its original arithmetic.
#[test]
fn warm_keys_skip_work_and_distinguish_all_inputs() {
    let cache = Cache::default();
    let calls = std::cell::Cell::new(0);
    for (text, size, bold) in [
        ("label", 0.0_f32, false),
        ("label", -0.0, false),
        ("label", 10.0, false),
        ("label", f32::from_bits(10.0_f32.to_bits() + 1), false),
        ("label", 10.0, true),
        ("Label", 10.0, false),
    ] {
        let expected = text.chars().count() as f32 * size * 0.6;
        let actual = cache.measure(text, size, bold, || {
            calls.set(calls.get() + 1);
            expected
        });
        assert_eq!(actual.to_bits(), expected.to_bits());
        assert_eq!(cache.measure(text, size, bold, || panic!("a matching key must hit")).to_bits(), expected.to_bits());
    }
    assert_eq!(calls.get(), 6);
}

/// Non-finite sizes and oversized UTF-8 strings are measured every time without retained keys.
/// The exact byte threshold still admits a cache hit, independent of the glyph count.
#[test]
fn bypassed_inputs_do_not_allocate_entries() {
    let cache = Cache::default();
    let long = "中".repeat(MAX_TEXT_BYTES / 3 + 1);
    let calls = std::cell::Cell::new(0);
    for (text, size) in [("label", f32::NAN), ("label", f32::INFINITY), ("label", f32::NEG_INFINITY), (&long, 10.0)] {
        for _ in 0..2 {
            let width = cache.measure(text, size, false, || {
                calls.set(calls.get() + 1);
                f32::from_bits(0x7fc0_1234)
            });
            assert_eq!(width.to_bits(), 0x7fc0_1234);
        }
    }
    assert_eq!(calls.get(), 8);
    assert!(cache.shards.iter().all(|s| s.lock().unwrap().entries.capacity() == 0));
    let text = "x".repeat(MAX_TEXT_BYTES);
    assert_eq!(cache.measure(&text, 10.0, false, || 123.0), 123.0);
    assert_eq!(cache.measure(&text, 10.0, false, || panic!("threshold input must hit")), 123.0);
}

/// Forced hash collisions cannot return another string, size or weight's width; replacement
/// also removes the old string capacity from the retained-size estimate.
#[test]
fn collisions_are_misses_and_replacements_are_accounted() {
    let mut widths = Widths::default();
    widths.insert(7, "a longer label", 0, false, 9.0);
    assert_eq!(widths.get(7, "a longer label", 0, false), Some(9.0));
    assert_eq!(widths.get(7, "another label", 0, false), None);
    assert_eq!(widths.get(7, "a longer label", (-0.0_f32).to_bits(), false), None);
    assert_eq!(widths.get(7, "a longer label", 0, true), None);
    widths.insert(7, "b", 0, true, 2.0);
    assert_eq!(widths.entries.len(), 1);
    assert_eq!(widths.strings, widths.entries.get(&7).unwrap().text.capacity());
    assert_eq!(widths.get(7, "a longer label", 0, false), None);
    assert_eq!(widths.get(7, "b", 0, true), Some(2.0));
}

/// Budget pressure releases both strings and hash-table capacity, then accepts fresh entries.
/// Each post-insert estimate is checked; the policy does not claim a transient allocation cap.
#[test]
fn budget_eviction_releases_capacity_and_allows_reuse() {
    // Empty strings force the table capacity alone to cross the budget; long ones charge strings too.
    for len in [0, MAX_TEXT_BYTES] {
        let mut widths = Widths::default();
        let text = "x".repeat(len);
        let mut evicted = false;
        for key in 0..4096_u32 {
            widths.insert(u64::from(key), &text, key, false, 1.0);
            assert!(widths.retained_bytes() <= SHARD_BYTES);
            if widths.entries.is_empty() {
                evicted = true;
                assert_eq!(widths.strings, 0);
                assert_eq!(widths.entries.capacity(), 0);
                break;
            }
        }
        assert!(evicted, "the fixture must cross the retention budget");
        widths.insert(1, "fresh", 0, false, 42.0);
        assert_eq!(widths.get(1, "fresh", 0, false), Some(42.0));
    }
}

/// A measurement may reenter the cache because the expensive callback runs without its lock.
/// A failed callback publishes no entry and leaves the same key usable on a later attempt.
#[test]
fn reentrant_and_failed_measurements_leave_the_cache_usable() {
    let cache = Cache::default();
    assert_eq!(cache.measure("same", 10.0, false, || cache.measure("same", 10.0, false, || 17.0)), 17.0);
    let failed = std::panic::catch_unwind(|| cache.measure("failed", 10.0, false, || panic!("measurement failed")));
    assert!(failed.is_err());
    assert_eq!(cache.measure("failed", 10.0, false, || 23.0), 23.0);
    assert_eq!(cache.measure("failed", 10.0, false, || panic!("retry must have been cached")), 23.0);
}

/// Poisoning a cache lock cannot make later map measurements panic or discard valid widths.
/// All shards are poisoned to exercise the public lookup path without depending on its hash.
#[test]
fn poisoned_shards_keep_measurements_available() {
    let cache = Cache::default();
    assert_eq!(cache.measure("retained", 10.0, false, || 19.0), 19.0);
    for shard in &cache.shards {
        let result = std::panic::catch_unwind(|| {
            let _guard = shard.lock().unwrap();
            panic!("poison the test cache");
        });
        assert!(result.is_err() && shard.is_poisoned());
    }
    assert_eq!(cache.measure("retained", 10.0, false, || panic!("the retained entry must hit")), 19.0);
    assert_eq!(cache.measure("new", 10.0, false, || 29.0), 29.0);
    assert_eq!(cache.measure("new", 10.0, false, || panic!("the new entry must hit")), 29.0);
}

/// Independent sizes and weights stay correct under concurrent cold misses and warm reads.
/// Local cache state avoids dependence on parallel tests' global-cache evictions.
#[test]
fn concurrent_measurements_preserve_results() {
    let cache = Cache::default();
    std::thread::scope(|scope| {
        for worker in 0..8 {
            let cache = &cache;
            scope.spawn(move || {
                for n in 0..64 {
                    let text = format!("AV road 中 {}", n % 4);
                    let size = 8.0 + worker as f32;
                    let bold = worker % 2 == 0;
                    let expected = original_measure(&text, size, bold).to_bits();
                    assert_eq!(cache.measure(&text, size, bold, || original_measure(&text, size, bold)).to_bits(), expected);
                    assert_eq!(cache.measure(&text, size, bold, || panic!("warm concurrent key must hit")).to_bits(), expected);
                }
            });
        }
    });
}
