//! A tokio runtime on its own thread (design §6.2.2).
//!
//! reqwest needs tokio, GPUI's executor is not tokio. Requests run here and
//! hand their events over channels that any executor can poll.

use std::future::Future;
use std::io;
use std::sync::Arc;

use tokio::sync::oneshot;
use tokio::task::AbortHandle;

/// Handle to the runtime thread. Clones share the thread; it stops when the
/// last clone is dropped, aborting whatever is still running on it.
#[derive(Clone, Debug)]
pub struct Runtime {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    handle: tokio::runtime::Handle,
    /// Dropping the sender ends the thread's `block_on`.
    _shutdown: oneshot::Sender<()>,
}

impl Runtime {
    /// Starts the thread.
    pub fn new() -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .thread_name("mw-llm")
            .build()?;
        let handle = runtime.handle().clone();
        let (shutdown, stopped) = oneshot::channel::<()>();
        std::thread::Builder::new()
            .name("mw-llm".into())
            .spawn(move || {
                runtime.block_on(async {
                    let _ = stopped.await;
                });
                // Don't wait for blocking work such as DNS lookups.
                runtime.shutdown_background();
            })?;
        Ok(Self {
            inner: Arc::new(Inner {
                handle,
                _shutdown: shutdown,
            }),
        })
    }

    pub(crate) fn spawn<F>(&self, future: F) -> AbortHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.inner.handle.spawn(future).abort_handle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_tasks_off_the_calling_thread() {
        let runtime = Runtime::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let caller = std::thread::current().id();
        runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            tx.send(std::thread::current().id() != caller).unwrap();
        });
        assert!(rx.recv().unwrap());
    }

    #[test]
    fn aborting_stops_a_task() {
        let runtime = Runtime::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let task = runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            drop(tx);
        });
        task.abort();
        // The sender is dropped with the aborted task, long before 60 s.
        assert!(rx.recv().is_err());
    }
}
