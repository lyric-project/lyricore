use crate::error::Result;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::runtime::{Builder, Handle, Runtime as TokioRawRuntime};

#[derive(Clone, Debug)]
pub struct TokioRuntime {
    pub runtime: Arc<TokioRawRuntime>,
}

impl TokioRuntime {
    pub fn new(worker_threads: usize) -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(worker_threads)
            .enable_all()
            .thread_name_fn(|| {
                static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
                let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
                format!("lyricore-event-thread-{}", id)
            })
            .build()
            .map_err(|e| crate::error::LyricoreActorError::Runtime(e.to_string()))?;
        Ok(Self {
            runtime: Arc::new(runtime),
        })
    }

    pub fn handle(&self) -> Handle {
        self.runtime.handle().clone()
    }

    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.runtime)
    }

    pub fn weak_count(&self) -> usize {
        Arc::weak_count(&self.runtime)
    }
}

impl Drop for TokioRuntime {
    fn drop(&mut self) {
        let strong_count = Arc::strong_count(&self.runtime);
        let weak_count = Arc::weak_count(&self.runtime);
        // Last reference being dropped, log the stack trace
        if strong_count == 1 {
            tracing::warn!(
                "Last TokioRuntime reference being dropped! Stack trace: {}, strong_count: {}, weak_count: {}",
                std::backtrace::Backtrace::capture(),
                strong_count,
                weak_count
            );
        }
    }
}
