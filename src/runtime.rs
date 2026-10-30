//! Tokio ↔ GPUI bridge.
//!
//! GPUI has its own executor and no Tokio, but `tokio::process` needs one. A dedicated
//! multi-thread runtime lives in a `OnceLock`; run IO on it with [`run`] and await the result from
//! a GPUI task:
//!
//! ```ignore
//! cx.spawn(async move |this, cx| {
//!     let out = runtime::run(async move { backend::spm::installed(&dir).await }).await;
//!     this.update(cx, |this, cx| { /* store result */ cx.notify(); }).ok();
//! }).detach();
//! ```

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("agentory-tokio")
            .enable_all()
            .build()
            .expect("failed to start the Tokio runtime")
    })
}

/// Run `fut` on the Tokio runtime and await its output from any executor (including GPUI's).
pub async fn run<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static) -> T {
    runtime()
        .spawn(fut)
        .await
        .expect("tokio task panicked or was cancelled")
}
