//! Calls for a host that lives on another thread (ai-integration design
//! §5.1): the running application keeps its projects and structures on its
//! main thread, so the server hands each call there and waits for the
//! answer, instead of calling the host itself.

use std::sync::Arc;

use mw_automation::Host;
use tokio::sync::mpsc;

use crate::Access;

/// A call's work on the host.
pub(crate) type Job = Box<dyn FnOnce(&mut dyn Host) + Send>;

/// Makes the two ends: the dispatcher for servers ([`crate::Server::dispatching`],
/// [`crate::listen`]) and the calls for the host's thread to run.
pub fn dispatcher() -> (Dispatcher, Calls) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (Dispatcher(sender), Calls(receiver))
}

/// Hands calls to the host's thread. Cheap to clone; every server holds
/// one.
#[derive(Clone, Debug)]
pub struct Dispatcher(mpsc::UnboundedSender<Call>);

impl Dispatcher {
    /// Hands over a call; one the host's thread will never run (it has
    /// stopped) is dropped, which its server reports as an error.
    pub(crate) fn send(&self, access: Arc<Access>, job: Job) {
        let _ = self.0.send(Call { access, job });
    }
}

/// The calls servers hand over, in order. Polled on any executor, such as
/// GPUI's on the main thread.
#[derive(Debug)]
pub struct Calls(mpsc::UnboundedReceiver<Call>);

impl Calls {
    /// The next call; `None` once every dispatcher is gone.
    pub async fn next(&mut self) -> Option<Call> {
        self.0.recv().await
    }

    /// The next call if one is waiting.
    pub fn try_next(&mut self) -> Option<Call> {
        self.0.try_recv().ok()
    }
}

/// One call for the host.
pub struct Call {
    access: Arc<Access>,
    job: Job,
}

impl Call {
    /// What the client that made the call may open: the directories its
    /// command line allows (design §7.2).
    pub fn access(&self) -> &Access {
        &self.access
    }

    /// Runs the call on the host; the server gets the answer. Panics in the
    /// capability are caught and answered as errors.
    pub fn run(self, host: &mut dyn Host) {
        (self.job)(host)
    }
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call")
            .field("access", &self.access)
            .finish_non_exhaustive()
    }
}
