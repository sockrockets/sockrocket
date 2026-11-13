use std::future::Future;
use std::io;
use std::pin::{Pin, pin};
use tokio::runtime::{Builder, Runtime};

pub(super) struct CgiRuntime(Runtime);

impl CgiRuntime {
    pub(super) fn new() -> io::Result<Self> {
        Builder::new_current_thread().enable_all().build().map(Self)
    }

    pub(super) fn block_on<F: Future>(&self, future: F) -> F::Output {
        let mut result = None;
        self.drive(pin!(async { result = Some(future.await) }));
        result.expect("CGI task completed without a result")
    }

    // One Tokio scheduler instantiation serves all synchronous CGI actions.
    // Only the outer task is type-erased; its storage stays on the caller's stack.
    #[inline(never)]
    fn drive(&self, task: Pin<&mut dyn Future<Output = ()>>) {
        self.0.block_on(task);
    }
}
