use crate::executor::{Handle, JoinHandle};
use core::future::Future;

crate::scope::define!(scope, Handle);

pub fn spawn<F: 'static + Future<Output = T> + Send, T: 'static + Send>(
    future: F,
) -> JoinHandle<T> {
    scope::borrow_with(|h| h.spawn(future))
}

pub mod primary {
    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicU64, Ordering};

    pub fn spawn<F: 'static + Future<Output = T> + Send, T: 'static + Send>(
        future: F,
    ) -> JoinHandle<T> {
        scope::borrow_with(|h| h.spawn_primary(future))
    }

    #[derive(Debug)]
    pub struct Guard(Arc<AtomicU64>);

    impl Guard {
        pub(crate) fn new(count: Arc<AtomicU64>) -> Self {
            count.fetch_add(1, Ordering::SeqCst);
            Self(count)
        }
    }

    impl Clone for Guard {
        fn clone(&self) -> Self {
            self.0.fetch_add(1, Ordering::SeqCst);
            Self(self.0.clone())
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    pub fn guard() -> Guard {
        scope::borrow_with(|h| h.primary_guard())
    }

    pub async fn create<F: Future<Output = T>, T>(future: F) -> T {
        // hold a primary guard
        let guard = guard();
        let value = future.await;
        drop(guard);
        value
    }
}
