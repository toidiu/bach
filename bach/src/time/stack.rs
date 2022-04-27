use super::{
    bitset::Bitset,
    entry::{Entry, Queue},
};
use arr_macro::arr;
use core::{fmt, marker::PhantomData};

pub struct Stack<E: Entry> {
    slots: [E::Queue; 256],
    occupied: Bitset,
    current: u8,
    entry: PhantomData<E>,
}

impl<E: Entry> Default for Stack<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Entry> fmt::Debug for Stack<E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stack")
            .field("occupied", &self.occupied.len())
            .field("current", &self.current)
            .finish()
    }
}

impl<E: Entry> Stack<E> {
    pub fn new() -> Self {
        let slots = arr![E::Queue::new(); 256];
        Self {
            slots,
            occupied: Default::default(),
            current: 0,
            entry: PhantomData,
        }
    }

    pub fn current(&self) -> u8 {
        self.current
    }

    pub fn is_empty(&self) -> bool {
        self.occupied.is_empty()
    }

    pub fn insert(&mut self, index: u8, entry: E) {
        self.occupied.insert(index);
        let list = self.slot_mut(index);
        list.push(entry);
    }

    fn skip(&mut self) {
        if let Some(next) = self.occupied.next_occupied(self.current) {
            self.current = next;
        } else {
            self.current = 0;
        }
    }

    pub fn tick(&mut self, can_skip: bool) -> (E::Queue, bool) {
        self.current = self.current.wrapping_add(1);
        if can_skip {
            self.skip();
        }
        let slot = self.take();
        (slot, self.current == 0)
    }

    pub fn take(&mut self) -> E::Queue {
        let current = self.current;
        self.occupied.remove(current);
        self.slot_mut(current).take()
    }

    #[inline]
    pub fn close<F: FnMut(E)>(&mut self, mut close: F) {
        if self.occupied.is_empty() {
            return;
        }

        let mut idx = 0;
        loop {
            let queue = self.slot_mut(idx);
            while let Some(entry) = queue.pop() {
                close(entry);
            }

            self.occupied.remove(idx);

            if let Some(next_idx) = self.occupied.next_occupied(idx) {
                idx = next_idx;
            } else {
                break;
            }
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.current = 0;
    }

    fn slot_mut(&mut self, index: u8) -> &mut E::Queue {
        unsafe { self.slots.get_unchecked_mut(index as usize) }
    }
}
