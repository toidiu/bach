use async_task::{Runnable, Task};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use flume::{Receiver, Sender};

pub struct JoinHandle<Output>(Option<Task<Output>>);

impl<Output> JoinHandle<Output> {
    pub fn cancel(mut self) {
        if let Some(task) = self.0.take() {
            drop(task);
        }
    }

    pub async fn stop(mut self) -> Option<Output> {
        if let Some(task) = self.0.take() {
            task.cancel().await
        } else {
            None
        }
    }
}

impl<O> Future for JoinHandle<O> {
    type Output = O;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0.as_mut().unwrap()).poll(cx)
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.detach();
        }
    }
}

pub struct Executor<Environment> {
    environment: Environment,
    queue: Receiver<Runnable>,
    handle: Handle,
}

impl<E: Environment> Executor<E> {
    pub fn new<F: FnOnce(&Handle) -> E>(create_env: F, queue_size: Option<usize>) -> Self {
        let (sender, queue) = if let Some(queue_size) = queue_size {
            flume::bounded(queue_size)
        } else {
            flume::unbounded()
        };

        let handle = Handle(sender);

        let environment = create_env(&handle);

        Self {
            environment,
            queue,
            handle,
        }
    }

    pub fn spawn<F, Output>(&mut self, future: F) -> JoinHandle<Output>
    where
        F: Future<Output = Output> + Send + 'static,
        Output: Send + 'static,
    {
        self.handle.spawn(future)
    }

    pub fn tick(&mut self) -> Poll<()> {
        let tasks = self.queue.drain().map(|runnable| {
            move || {
                if runnable.run() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }
        });
        self.environment.run(tasks)
    }

    pub fn drain(&mut self) {
        while !self.queue.is_empty() {
            while self.tick() == Poll::Ready(()) {}
        }
    }

    pub fn block_on<T, Output, F>(&mut self, task: T, mut wait: F) -> Output
    where
        T: 'static + Future<Output = Output> + Send,
        Output: 'static + Send,
        F: FnMut(&mut Self),
    {
        use core::task::{RawWaker, RawWakerVTable, Waker};

        const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        unsafe fn clone(ptr: *const ()) -> RawWaker {
            RawWaker::new(ptr, &VTABLE)
        }
        unsafe fn noop(_ptr: *const ()) {
            // noop
        }

        let mut task = self.spawn(task);
        let waker = unsafe { Waker::from_raw(clone(core::ptr::null())) };
        let mut ctx = Context::from_waker(&waker);

        self.drain();

        loop {
            wait(self);
            self.drain();

            if let Poll::Ready(value) = Pin::new(&mut task).poll(&mut ctx) {
                return value;
            }
        }
    }

    pub fn environment(&mut self) -> &mut E {
        &mut self.environment
    }
}

#[derive(Clone)]
pub struct Handle(Sender<Runnable>);

impl Handle {
    pub fn spawn<F, Output>(&self, future: F) -> JoinHandle<Output>
    where
        F: Future<Output = Output> + Send + 'static,
        Output: Send + 'static,
    {
        let sender = self.0.clone();

        let (runnable, task) = async_task::spawn(future, move |runnable| {
            sender.send(runnable).expect("pending task limit exhausted");
        });

        // queue the initial poll
        runnable.schedule();

        JoinHandle(Some(task))
    }
}

pub trait Environment {
    fn run<Tasks, F>(&mut self, tasks: Tasks) -> Poll<()>
    where
        Tasks: Iterator<Item = F> + Send,
        F: 'static + FnOnce() -> Poll<()> + Send;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::eprint;

    pub fn executor() -> Executor<Env> {
        Executor::new(|_| Env::default(), None)
    }

    #[derive(Default)]
    pub struct Env;

    impl super::Environment for Env {
        fn run<Tasks, F>(&mut self, tasks: Tasks) -> Poll<()>
        where
            Tasks: Iterator<Item = F>,
            F: 'static + FnOnce() -> Poll<()> + Send,
        {
            let mut is_ready = false;
            for task in tasks {
                is_ready |= task().is_ready();
            }
            if is_ready {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    #[derive(Default)]
    struct Yield(bool);

    impl Future for Yield {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if core::mem::replace(&mut self.0, true) {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    #[test]
    fn basic_test() {
        let mut executor = executor();

        executor.spawn(async {
            Yield::default().await;
            eprint!("hello");
            Yield::default().await;
        });

        let exclaimation = async {
            Yield::default().await;
            eprint!("!!!!!");
            Yield::default().await;
        };
        executor.spawn(async {
            Yield::default().await;
            eprint!("world");
            Yield::default().await;
            exclaimation.await;
            Yield::default().await;
        });

        while executor.tick() == Poll::Ready(()) {}
    }
}
