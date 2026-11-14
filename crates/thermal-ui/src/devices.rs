//! Watches for wired printers coming and going.
//!
//! A background thread re-runs `tspl_core::discover` every few seconds (and on
//! demand) and reports only when the result changes, so plugging the printer
//! in or switching it on shows up, and is connected, without a Refresh click.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use tspl_core::discover::{discover, Discovery};

/// How often to look again.
const INTERVAL: Duration = Duration::from_secs(3);

/// Handle to the watcher.
pub struct Watcher {
    /// Send `()` to rescan now.
    pub refresh: Sender<()>,
    /// Changed discovery results.
    pub updates: Receiver<Discovery>,
}

/// Starts the watcher. `wake` is called after every update.
#[must_use]
pub fn spawn(wake: impl Fn() + Send + 'static) -> Watcher {
    let (refresh_tx, refresh_rx) = channel::<()>();
    let (update_tx, update_rx) = channel();

    // The thread ends when the UI drops `refresh`.
    std::thread::Builder::new()
        .name("discovery".to_string())
        .spawn(move || {
            let mut last: Option<Discovery> = None;
            loop {
                let current = discover();
                if last.as_ref() != Some(&current) {
                    if update_tx.send(current.clone()).is_err() {
                        return;
                    }
                    wake();
                    last = Some(current);
                }
                match refresh_rx.recv_timeout(INTERVAL) {
                    Ok(()) => {
                        // A forced refresh always reports, so the UI can say so.
                        last = None;
                        while refresh_rx.try_recv().is_ok() {}
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .expect("spawn discovery thread");

    Watcher {
        refresh: refresh_tx,
        updates: update_rx,
    }
}
