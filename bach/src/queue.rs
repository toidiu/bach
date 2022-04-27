use alloc::{collections::VecDeque, sync::Arc};
use parking_lot::Mutex;

type Inner<T> = Arc<Mutex<Option<VecDeque<T>>>>;

pub fn create_queue<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Mutex::new(Some(VecDeque::new())));
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
        if let Some(queue) = self.0.lock().as_mut() {
            queue.push_back(value);
        }
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
            if let Some(queue) = self.inner.lock().as_mut() {
                core::mem::swap(queue, &mut self.drain);
            }
        }

        self.drain.drain(..)
    }

    pub fn close(&mut self) -> Option<VecDeque<T>> {
        self.inner.lock().take()
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.close();
    }
}
