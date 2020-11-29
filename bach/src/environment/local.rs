use crate::executor::Handle;
use core::task::Poll;

pub struct Threaded<Scope> {
    scope: Scope,
}

pub struct Runner {}

impl<Scope> super::Environment for Threaded<Scope> {
    type Runner = Runner;

    fn with<F: Fn(Self::Runner)>(&mut self, handle: &Handle, f: F) -> Poll<()> {
        //
    }
}

impl super::Runner for Runner {
    fn run<F: FnOnce() -> bool + Send + 'static>(&mut self, run: F) {
        // TODO
    }
}
