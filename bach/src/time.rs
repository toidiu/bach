use alloc::sync::Arc;
use core::{
    fmt,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU64, Ordering},
    task::{Context, Poll},
    time::Duration,
};
use flume::{Receiver, Sender};

mod bitset;
mod entry;
mod stack;
mod wheel;

use entry::{ArcEntry, Entry};

pub struct Scheduler {
    wheel: wheel::Wheel,
    handle: Handle,
    queue: Receiver<ArcEntry>,
}

impl fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Scheduler")
            .field("now", &self.handle.now())
            .field("wheel", &self.wheel)
            .field("queue", &self.queue.len())
            .finish()
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new(Some(1024), Duration::from_millis(1))
    }
}

impl Scheduler {
    /// Creates a new Scheduler
    pub fn new(capacity: Option<usize>, tick_duration: Duration) -> Self {
        let (sender, queue) = if let Some(capacity) = capacity {
            flume::bounded(capacity)
        } else {
            flume::unbounded()
        };
        let handle = Handle::new(sender, tick_duration);

        Self {
            wheel: Default::default(),
            handle,
            queue,
        }
    }

    /// Returns a handle that can be easily cloned
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Returns the amount of time until the next task
    ///
    /// An implementation may sleep for the duration.
    pub fn advance(&mut self) -> Option<Duration> {
        self.update();

        let ticks = self.wheel.advance()?;
        let time = self.handle.advance(ticks);

        Some(time)
    }

    /// Wakes all of the expired tasks
    pub fn wake(&mut self) -> usize {
        self.wheel.wake()
    }

    /// Move the queued entries into the wheel
    fn update(&mut self) {
        for entry in self.queue.try_iter() {
            self.wheel.insert(entry);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Handle(Arc<InnerHandle>);

impl Handle {
    fn new(sender: Sender<ArcEntry>, tick_duration: Duration) -> Self {
        let nanos_per_tick = tick_duration.max(Duration::from_nanos(1)).as_nanos() as u64;
        let inner = InnerHandle {
            ticks: AtomicU64::new(0),
            sender,
            nanos_per_tick: AtomicU64::new(nanos_per_tick),
        };
        Self(Arc::new(inner))
    }

    /// Returns a future that sleeps for the given duration
    pub fn delay(&self, duration: Duration) -> Timer {
        // Assuming a tick duratin of 1ns, this will truncate out at about 584
        // years. Hopefully no one needs that :)
        let ticks = (duration.as_nanos() / self.nanos_per_tick() as u128) as u64;

        let entry = Entry::new(ticks, false);
        let handle = self.clone();
        Timer { handle, entry }
    }

    /// Returns the current amount of time that has passed for this scheduler
    pub fn now(&self) -> Duration {
        let mut ticks = self.ticks();
        ticks *= self.nanos_per_tick();
        Duration::from_nanos(ticks)
    }

    fn advance(&self, mut ticks: u64) -> Duration {
        if cfg!(test) {
            self.0
                .ticks
                .load(Ordering::SeqCst)
                .checked_add(ticks)
                .expect("tick overflow");
        }
        self.0.ticks.fetch_add(ticks, Ordering::SeqCst);
        ticks *= self.nanos_per_tick();
        Duration::from_nanos(ticks)
    }

    fn ticks(&self) -> u64 {
        self.0.ticks.load(Ordering::SeqCst)
    }

    fn nanos_per_tick(&self) -> u64 {
        self.0.nanos_per_tick.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
struct InnerHandle {
    ticks: AtomicU64,
    sender: Sender<ArcEntry>,
    nanos_per_tick: AtomicU64,
}

impl Handle {
    fn register(&self, entry: &ArcEntry) {
        self.0.sender.send(entry.clone()).expect("send queue full")
    }
}

/// A future that sleeps a task for a duration
pub struct Timer {
    handle: Handle,
    entry: ArcEntry,
}

impl Timer {
    /// Cancels the timer
    pub fn cancel(&mut self) {
        self.entry.cancel();
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Future for Timer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        // check condition before to avoid needless registration
        if self.entry.take_expired() {
            return Poll::Ready(());
        }

        // register the waker with the entry
        self.entry.register(cx.waker());

        // check condition after registration to avoid loss of notification
        if self.entry.take_expired() {
            return Poll::Ready(());
        }

        // register the timer with the handle
        if self.entry.should_register() {
            self.handle.register(&self.entry);
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor;
    use alloc::{rc::Rc, vec::Vec};
    use bolero::{check, generator::*};
    use core::cell::Cell;

    fn executor() -> executor::Executor<Env> {
        executor::Executor::new(Env::default(), None)
    }

    #[derive(Default)]
    struct Env;

    impl crate::executor::Environment for Env {
        type Runner = Runner;

        fn with<F: Fn(Self::Runner)>(&mut self, _handle: &executor::Handle, f: F) -> Poll<()> {
            let count = Rc::new(Cell::new(false));
            let runner = Runner(count.clone());
            f(runner);
            if count.get() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        }
    }

    #[derive(Default)]
    struct Runner(Rc<Cell<bool>>);

    impl crate::executor::Runner for Runner {
        fn run<F: FnOnce() -> bool + Send + 'static>(&mut self, run: F) {
            if run() {
                self.0.set(true);
            }
        }
    }

    async fn delay(handle: Handle, count: usize, delay: Duration) {
        let nanos_per_tick = handle.nanos_per_tick() as u128;
        for _ in 0..count {
            // get the time before the delay
            let now = handle.now();

            // await the delay
            handle.delay(delay).await;

            // get the time that has passed on the clock and make sure it matches the amount that
            // was delayed
            let actual = handle.now();
            let expected = now + delay;
            assert_eq!(
                actual.as_nanos() / nanos_per_tick,
                expected.as_nanos() / nanos_per_tick,
                "actual: {:?}, expected: {:?}",
                actual,
                expected
            );
        }
    }

    fn test_helper(delays: &[(u8, Duration)], min_time: Duration) {
        std::eprintln!();
        let mut scheduler = Scheduler::new(None, min_time);
        let mut executor = executor();

        let handle = scheduler.handle();

        for (count, duration) in delays.as_ref().iter() {
            executor.spawn(delay(handle.clone(), *count as usize, *duration));
        }

        let mut total = Duration::from_secs(0);

        loop {
            while executor.tick() == Poll::Pending {}

            if let Some(expiration) = scheduler.advance() {
                total += expiration;
                scheduler.wake();
            } else if scheduler.wake() == 0 {
                // there are no remaining tasks to execute
                break;
            }
        }

        assert!(executor.tick().is_ready(), "the task list should be empty");
        assert!(
            scheduler.advance().is_none(),
            "the scheduler should be empty"
        );
        assert_eq!(
            total,
            handle.now(),
            "the current time should reflect the total number of sleep durations"
        );

        drop(scheduler);
        drop(executor);

        assert_eq!(
            Arc::strong_count(&handle.0),
            1,
            "the scheduler should cleanly shut down"
        );
    }

    #[test]
    fn timer_test() {
        let min_time = Duration::from_nanos(1).as_nanos() as u64;
        let max_time = Duration::from_secs(3600).as_nanos() as u64;

        let delay = (min_time..max_time).map_gen(Duration::from_nanos);
        let count = 0u8..3;
        let delays = gen::<Vec<_>>().with().values((count, delay));

        let min_duration = (Duration::from_nanos(1).as_nanos()
            ..Duration::from_millis(10).as_nanos())
            .map_gen(|v| Duration::from_nanos(v as _));

        check!()
            .with_generator((delays, min_duration))
            .for_each(|(delays, min_duration)| test_helper(delays, *min_duration));
    }
}
