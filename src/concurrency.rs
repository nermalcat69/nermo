use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Run `f` over every item in `items` using up to `concurrency` OS threads.
/// Latency-bound work (network requests) benefits from this even without
/// async I/O, since most of the wait is idle time, not CPU.
///
/// Returns the first error encountered, if any. Once an error is recorded no
/// new items are started, though in-flight ones still finish.
pub fn parallel_for_each<T, F>(items: &[T], concurrency: usize, f: F) -> Result<()>
where
    T: Sync,
    F: Fn(&T) -> Result<()> + Sync,
{
    if items.is_empty() {
        return Ok(());
    }
    let next = AtomicUsize::new(0);
    let first_error: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let workers = concurrency.max(1).min(items.len());

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if first_error.lock().unwrap().is_some() {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(item) = items.get(i) else { break };
                if let Err(e) = f(item) {
                    let mut slot = first_error.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(e);
                    }
                }
            });
        }
    });

    match first_error.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn visits_every_item_exactly_once() {
        let items: Vec<u32> = (0..200).collect();
        let seen = StdMutex::new(Vec::new());
        parallel_for_each(&items, 8, |i| {
            seen.lock().unwrap().push(*i);
            Ok(())
        })
        .unwrap();

        let mut seen = seen.into_inner().unwrap();
        seen.sort_unstable();
        assert_eq!(seen, items);
    }

    #[test]
    fn propagates_first_error_without_hanging() {
        let items = vec![1, 2, 3, 4, 5];
        let err = parallel_for_each(&items, 4, |i| {
            if *i == 3 {
                bail!("boom at {i}");
            }
            Ok(())
        })
        .unwrap_err();
        assert!(err.to_string().contains("boom at 3"));
    }

    #[test]
    fn concurrency_is_clamped_to_item_count() {
        // Should not panic or deadlock when concurrency exceeds the work available.
        let items = vec![1];
        parallel_for_each(&items, 64, |_| Ok(())).unwrap();
    }
}
