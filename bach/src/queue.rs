use alloc::{collections::VecDeque, sync::Arc};
use parking_lot::Mutex;

type Inner<T> = Arc<Mutex<VecDeque<T>>>;

pub fn create_queue<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Mutex::new(VecDeque::new()));
    let sender = Sender(inner.clone());
    let receiver = Receiver {
        inner,
        drain: VecDeque::new(),
    };
    (sender, receiver)
}

#[derive(Debug)]
pub struct Sender<T>(Inner<T>);

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Sender<T> {
    pub fn send(&self, value: T) {
        self.0.lock().push_back(value)
    }
}

#[derive(Debug)]
pub struct Receiver<T> {
    inner: Inner<T>,
    drain: VecDeque<T>,
}

impl<T> Receiver<T> {
    pub fn drain(&mut self) -> alloc::collections::vec_deque::Drain<T> {
        if self.drain.is_empty() {
            core::mem::swap(&mut *self.inner.lock(), &mut self.drain);
        }

        self.drain.drain(..)
    }
}
