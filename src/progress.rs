use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A minimal single-line spinner for a long-running phase (resolve,
/// download, link). Deliberately does nothing deep: no hook into resolver
/// or store internals, no per-item progress counting — just "this phase is
/// running, here's how long it's taken" on one line, cleared when the phase
/// finishes so it never clutters the real output that follows.
///
/// Only animates on a real terminal (`stdout().is_terminal()`); piped or
/// redirected output — scripts, logs, CI — gets exactly the plain
/// `"{label}..."` line this replaced, unchanged.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    animated: bool,
}

impl Spinner {
    pub fn start(label: &str) -> Self {
        let animated = std::io::stdout().is_terminal();
        if !animated {
            println!("{label}...");
            return Self { stop: Arc::new(AtomicBool::new(false)), handle: None, animated };
        }

        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let stop = stop.clone();
            let label = label.to_string();
            std::thread::spawn(move || {
                let start = Instant::now();
                let mut frame = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    print!("\r\x1b[2K{} {label} ({:.1}s)", FRAMES[frame % FRAMES.len()], start.elapsed().as_secs_f32());
                    let _ = std::io::stdout().flush();
                    frame += 1;
                    std::thread::sleep(Duration::from_millis(80));
                }
            })
        };
        Self { stop, handle: Some(handle), animated }
    }

    /// Stop and clear the spinner's line. Whatever the caller prints next
    /// (e.g. "Found 42 packages.") becomes the phase's visible result —
    /// this never prints a summary of its own.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if self.animated {
            print!("\r\x1b[2K");
            let _ = std::io::stdout().flush();
        }
    }
}
