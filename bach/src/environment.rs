use crate::executor::Handle;
use core::task::Poll;

pub mod local;
pub mod threaded;

pub trait Environment {
    type Runner: Runner;

    fn with<F: Fn(Self::Runner)>(&mut self, handle: &Handle, f: F) -> Poll<()>;
}

pub trait Runner {
    fn run<F: FnOnce() -> bool + Send + 'static>(&mut self, run: F);
}
