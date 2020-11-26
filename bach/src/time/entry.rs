use alloc::sync::Arc;
use core::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    task::Waker,
};
use futures::task::AtomicWaker;
use intrusive_collections::{intrusive_adapter, LinkedList, LinkedListLink};

intrusive_adapter!(pub Adapter = ArcEntry: Entry { link: LinkedListLink });

pub type List = LinkedList<Adapter>;

pub type ArcEntry = Arc<Entry>;

#[derive(Debug)]
pub struct Entry {
    waker: AtomicWaker,
    expired: AtomicBool,
    delay: u64,
    periodic: bool,
    start_tick: AtomicU64,
    link: LinkedListLink,
}

unsafe impl Send for Entry {}
unsafe impl Sync for Entry {}

impl Entry {
    pub fn new(delay: u64, periodic: bool) -> Arc<Self> {
        Arc::new(Entry {
            waker: AtomicWaker::new(),
            expired: AtomicBool::new(false),
            delay,
            periodic,
            start_tick: AtomicU64::new(0),
            link: LinkedListLink::new(),
        })
    }

    pub fn delay(&self) -> u64 {
        self.delay
    }

    pub fn start_tick(&self) -> u64 {
        self.start_tick.load(Ordering::SeqCst)
    }

    pub fn set_start_tick(&self, tick: u64) {
        self.start_tick.store(tick, Ordering::SeqCst);
    }

    pub fn wake(&self) -> bool {
        self.expired.store(true, Ordering::SeqCst);

        if self.periodic {
            self.waker.wake();
            return true;
        }

        if let Some(waker) = self.waker.take() {
            waker.wake();
        }

        false
    }

    pub fn is_registered(&self) -> bool {
        self.link.is_linked()
    }

    pub fn cancel(&self) {
        self.waker.take();
    }

    pub fn take_expired(&self) -> bool {
        self.expired.swap(false, Ordering::SeqCst)
    }

    pub fn register(&self, waker: &Waker) {
        self.waker.register(waker)
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.cancel();
    }
}
