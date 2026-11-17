//! Cleanup must outlive cancellation and shutdown of the calling Tokio runtime.
use std::sync::{Arc, Mutex};

pub(super) fn dispatch(work: impl FnOnce() + Send + 'static) {
    // Keep a recoverable copy of ownership if OS thread creation itself fails.
    let work = Arc::new(Mutex::new(Some(work)));
    let background = work.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("browser-cleanup".into())
        .spawn(move || {
            if let Some(work) = background.lock().unwrap_or_else(|e| e.into_inner()).take() {
                work();
            }
        })
    {
        eprintln!("start browser cleanup thread: {error}; cleaning up synchronously");
        if let Some(work) = work.lock().unwrap_or_else(|e| e.into_inner()).take() {
            work();
        }
    }
}
