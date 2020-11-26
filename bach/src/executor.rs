use async_task::{Runnable, Task};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use flume::{Receiver, Sender};

pub struct JoinHandle<Output>(Option<Task<Output>>);

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
    pub fn new(environment: E, queue_size: Option<usize>) -> Self {
        let (sender, queue) = if let Some(queue_size) = queue_size {
            flume::bounded(queue_size)
        } else {
            flume::unbounded()
        };

        Self {
            environment,
            queue,
            handle: Handle(sender),
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
        let queue = &self.queue;
        self.environment.with(&self.handle, |mut runner| {
            for runnable in queue.drain() {
                runner.run(|| runnable.run());
            }
        })
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
    type Runner: Runner;

    fn with<F: Fn(Self::Runner)>(&mut self, handle: &Handle, f: F) -> Poll<()>;
}

pub trait Runner {
    fn run<F: FnOnce() -> bool + Send + 'static>(&mut self, run: F);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use core::cell::Cell;
    use std::eprint;

    fn executor() -> Executor<Env> {
        Executor::new(Env::default(), None)
    }

    #[derive(Default)]
    struct Env;

    impl super::Environment for Env {
        type Runner = Runner;

        fn with<F: Fn(Self::Runner)>(&mut self, _handle: &Handle, f: F) -> Poll<()> {
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

    impl super::Runner for Runner {
        fn run<F: FnOnce() -> bool + Send + 'static>(&mut self, run: F) {
            if run() {
                self.0.set(true);
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

        while executor.tick() == Poll::Pending {}
    }
}
