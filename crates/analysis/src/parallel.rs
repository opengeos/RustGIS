//! Work spread over the machine's cores (#1252): a tool's independent items (features, raster
//! rows) made on several threads, in their order, as one thread would make them.
//!
//! The browser has no threads, so there everything runs on the calling thread, as it does below
//! a tool's threshold, where starting threads costs more than they save.

/// The most threads a tool uses.
const MAX_THREADS: usize = 16;
/// The items a thread takes at a time: few enough that expensive items lying together are shared
/// out, enough that taking them costs nothing.
pub(crate) const BATCH: usize = 16;
/// Each worker's stack, as large as the main thread's usually is.
const STACK_BYTES: usize = 8 << 20;

/// How many threads to make `items` items on: one below `min_items`, always one in the browser,
/// and no more than there are batches.
pub(crate) fn threads_for(items: usize, min_items: usize) -> usize {
    if cfg!(target_arch = "wasm32") || items < min_items {
        return 1;
    }
    std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_THREADS).min(items.div_ceil(BATCH)).max(1)
}

/// `f` on each of `items` with its index, the results in their order: on several threads when
/// there are at least `min_items` of them (one in the browser). Each thread makes its own
/// `scratch()` once and passes it to every `f` it runs, for buffers to reuse; the index lets a
/// raster tool map over its rows' numbers. The first error stops the threads after the items
/// they are making, and comes back instead of the results: on one thread the first in input
/// order, on several the earliest of those found before they stopped.
pub(crate) fn try_map<T: Sync, S, R: Send, E: Send>(
    items: &[T],
    min_items: usize,
    scratch: impl Fn() -> S + Sync,
    f: impl Fn(&mut S, usize, &T) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E> {
    try_map_on(items, threads_for(items.len(), min_items), scratch, f)
}

/// [`try_map`] on `threads` threads, this one among them. Each takes the next [`BATCH`] of items
/// until none are left or one has failed, so the work is shared however it is spread, and the
/// results go back in their places. A thread that can't start (a limit on threads, say) leaves
/// its share to the others; one that panics passes its panic on, as an item made here would.
pub(crate) fn try_map_on<T: Sync, S, R: Send, E: Send>(
    items: &[T],
    threads: usize,
    scratch: impl Fn() -> S + Sync,
    f: impl Fn(&mut S, usize, &T) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E> {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    if threads <= 1 || items.len() <= 1 {
        let mut s = scratch();
        return items.iter().enumerate().map(|(i, item)| f(&mut s, i, item)).collect();
    }
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let work = || {
        let mut s = scratch();
        let mut made = Vec::new();
        while !failed.load(Ordering::Relaxed) {
            let from = next.fetch_add(BATCH, Ordering::Relaxed);
            let Some(batch) = items.get(from..items.len().min(from + BATCH)).filter(|b| !b.is_empty()) else { break };
            for (k, item) in batch.iter().enumerate() {
                match f(&mut s, from + k, item) {
                    Ok(r) => made.push((from + k, r)),
                    Err(e) => {
                        failed.store(true, Ordering::Relaxed);
                        return Err((from + k, e));
                    }
                }
            }
        }
        Ok(made)
    };
    let mut parts = Vec::with_capacity(threads);
    std::thread::scope(|scope| {
        let others: Vec<_> =
            (1..threads).filter_map(|_| std::thread::Builder::new().stack_size(STACK_BYTES).spawn_scoped(scope, work).ok()).collect();
        parts.push(work());
        for h in others {
            // The worker's panic, raised again here as a serial run would raise it, for
            // `analysis::run` to catch.
            parts.push(h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)));
        }
    });
    let mut made: Vec<(usize, R)> = Vec::with_capacity(items.len());
    let mut first: Option<(usize, E)> = None;
    for part in parts {
        match part {
            Ok(m) => made.extend(m),
            Err((i, e)) => {
                if first.as_ref().is_none_or(|(j, _)| i < *j) {
                    first = Some((i, e));
                }
            }
        }
    }
    if let Some((_, e)) = first {
        return Err(e);
    }
    made.sort_unstable_by_key(|(i, _)| *i);
    Ok(made.into_iter().map(|(_, r)| r).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The infallible case, for the tests.
    fn map_on<T: Sync, S, R: Send>(
        items: &[T],
        threads: usize,
        scratch: impl Fn() -> S + Sync,
        f: impl Fn(&mut S, usize, &T) -> R + Sync,
    ) -> Vec<R> {
        try_map_on(items, threads, scratch, |s, i, item| Ok::<R, ()>(f(s, i, item))).unwrap_or_default()
    }

    fn map<T: Sync, S, R: Send>(
        items: &[T],
        min_items: usize,
        scratch: impl Fn() -> S + Sync,
        f: impl Fn(&mut S, usize, &T) -> R + Sync,
    ) -> Vec<R> {
        try_map(items, min_items, scratch, |s, i, item| Ok::<R, ()>(f(s, i, item))).unwrap_or_default()
    }

    #[test]
    fn items_made_on_several_threads_come_back_in_their_order() {
        let items: Vec<u64> = (0..1_000).collect();
        let alone: Vec<u64> = items.iter().map(|x| x * x + 1).collect();
        // Each item comes with its index, here equal to its value.
        let square = |_: &mut (), i: usize, x: &u64| {
            assert_eq!(i as u64, *x);
            x * x + 1
        };
        for threads in [1, 2, 3, 16, 64] {
            assert_eq!(map_on(&items, threads, || (), square), alone, "{threads} threads");
            assert_eq!(map_on(&items[..3], threads, || (), square), alone[..3], "{threads} threads, 3 items");
            assert!(map_on(&items[..0], threads, || (), square).is_empty());
        }
        // Below the threshold, and in the browser, one thread.
        assert_eq!(threads_for(10, 256), 1);
        assert_eq!(map(&items, 256, || (), square), alone);
    }

    #[test]
    fn each_thread_makes_its_scratch_once() {
        let made = std::sync::atomic::AtomicUsize::new(0);
        let items: Vec<usize> = (0..1_000).collect();
        // Each item records how many it saw before it on its thread, from that thread's scratch.
        let seen = map_on(
            &items,
            4,
            || {
                made.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                0usize
            },
            |count, _, _| {
                *count += 1;
                *count
            },
        );
        let scratches = made.load(std::sync::atomic::Ordering::Relaxed);
        assert!((1..=4).contains(&scratches), "{scratches}");
        // A thread's first item sees 1; a thread that found nothing left has no first item.
        let firsts = seen.iter().filter(|c| **c == 1).count();
        assert!((1..=scratches).contains(&firsts), "{firsts} of {scratches}");
    }

    #[test]
    fn a_workers_panic_reaches_the_caller() {
        // Only the workers panic, not this thread. Each item takes a moment, so workers get some,
        // unless none could start: then nothing panics, which is right too.
        let items: Vec<u32> = (0..200).collect();
        let caller = std::thread::current().id();
        let on_worker = std::sync::atomic::AtomicBool::new(false);
        let r = std::panic::catch_unwind(|| {
            map_on(
                &items,
                4,
                || (),
                |_, _, x| {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    if std::thread::current().id() != caller {
                        on_worker.store(true, std::sync::atomic::Ordering::Relaxed);
                        panic!("a worker's panic");
                    }
                    *x
                },
            )
        });
        assert_eq!(r.is_err(), on_worker.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn a_failing_item_stops_the_threads_and_its_error_comes_back() {
        let items: Vec<u32> = (0..1_000).collect();
        let f = |_: &mut (), _: usize, x: &u32| if *x == 500 || *x == 700 { Err(*x) } else { Ok(*x) };
        // On one thread, the first in input order.
        assert_eq!(try_map_on(&items, 1, || (), f), Err(500));
        for threads in [2, 3, 16] {
            assert!(matches!(try_map_on(&items, threads, || (), f), Err(500 | 700)), "{threads} threads");
            assert_eq!(try_map_on(&items, threads, || (), |_, _, x| Ok::<_, ()>(*x)), Ok(items.clone()), "{threads} threads");
        }
        // Items still waiting when one fails are left: far fewer than all of them run.
        let ran = std::sync::atomic::AtomicUsize::new(0);
        let r = try_map_on(
            &items,
            4,
            || (),
            |_, _, x| {
                ran.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if *x == 0 { Err(()) } else { Ok(()) }
            },
        );
        assert!(r.is_err());
        assert!(ran.load(std::sync::atomic::Ordering::Relaxed) < items.len());
    }

    #[test]
    fn no_more_threads_than_batches() {
        assert_eq!(threads_for(10, 256), 1);
        assert!(threads_for(20, 1) <= 2);
        assert!(threads_for(300, 1) <= 300usize.div_ceil(BATCH));
    }
}
