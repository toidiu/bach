use crate::executor::{Handle, JoinHandle};
use core::future::Future;

crate::scope::define!(scope, Handle);

pub fn spawn<F: 'static + Future<Output = T> + Send, T: 'static + Send>(
    future: F,
) -> JoinHandle<T> {
    scope::borrow_with(|h| h.spawn(future))
}

pub fn spawn_primary<F: 'static + Future<Output = T> + Send, T: 'static + Send>(
    future: F,
) -> JoinHandle<T> {
    scope::borrow_with(|h| h.spawn_primary(future))
}
