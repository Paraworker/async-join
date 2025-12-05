use std::{
    any::Any,
    fmt,
    panic::AssertUnwindSafe,
    pin::Pin,
    task::{Context, Poll},
};

use futures::{
    FutureExt,
    channel::oneshot::{self, Receiver, Sender},
    stream::{AbortHandle, AbortRegistration, Abortable},
};

/// Error returned by a [`JoinHandle`].
pub enum JoinError {
    /// The task was aborted by the [`JoinHandle`] or was dropped before it completed.
    Canceled,
    /// The task panicked.
    Panicked(Box<dyn Any + Send + 'static>),
}

impl fmt::Debug for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JoinError::Canceled => write!(f, "JoinError::Canceled"),
            JoinError::Panicked(_) => write!(f, "JoinError::Panicked(_)"),
        }
    }
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JoinError::Canceled => write!(f, "task canceled"),
            JoinError::Panicked(_) => write!(f, "task panicked"),
        }
    }
}

impl std::error::Error for JoinError {}

/// Result type returned by a [`JoinHandle`].
pub type JoinResult<T> = Result<T, JoinError>;

/// Handle to a joinable task.
///
/// Awaiting the `JoinHandle` waits for the task to complete and returns its result.
///
/// Dropping the `JoinHandle` detaches the task. The executor may continue to
/// poll the task to completion, but its result will be ignored.
pub struct JoinHandle<T> {
    /// Handle to abort the task.
    abort: AbortHandle,
    /// Receiver to get the task's result.
    rx: Receiver<JoinResult<T>>,
}

impl<T> JoinHandle<T> {
    /// Aborts the associated task.
    pub fn abort(&self) {
        self.abort.abort();
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = JoinResult<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        match Pin::new(&mut this.rx).poll(cx) {
            // The task has not yet produced a result.
            Poll::Pending => Poll::Pending,

            // Result received.
            Poll::Ready(Ok(res)) => Poll::Ready(res),

            // The sender was dropped without sending a result, treating it as canceled.
            Poll::Ready(Err(_canceled)) => Poll::Ready(Err(JoinError::Canceled)),
        }
    }
}

impl<T> fmt::Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinHandle")
            .field("abort", &self.abort)
            .field("rx", &self.rx)
            .finish()
    }
}

async fn task<F, T>(fut: F, tx: Sender<JoinResult<T>>, reg: AbortRegistration)
where
    F: Future<Output = T>,
{
    let res = Abortable::new(AssertUnwindSafe(fut).catch_unwind(), reg).await;

    // Map the result into `JoinResult`
    let res = match res {
        // Not aborted and panicked.
        Ok(Ok(value)) => Ok(value),

        // Not aborted, but panicked.
        Ok(Err(payload)) => Err(JoinError::Panicked(payload)),

        // Aborted.
        Err(_aborted) => Err(JoinError::Canceled),
    };

    // Send the result back to the `JoinHandle`.
    // Ignore errors, as it means the task is detached.
    let _ = tx.send(res);
}

/// Converts a future into a joinable task.
pub fn joinable<F, T>(fut: F) -> (impl Future<Output = ()>, JoinHandle<T>)
where
    F: Future<Output = T>,
{
    let (tx, rx) = oneshot::channel();
    let (abort, reg) = AbortHandle::new_pair();

    let task = task(fut, tx, reg);
    let join_handle = JoinHandle { abort, rx };

    (task, join_handle)
}
