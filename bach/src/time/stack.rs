use super::{
    bitset::Bitset,
    entry::{Adapter, ArcEntry, List},
};
use arr_macro::arr;
use core::fmt;

pub struct Stack {
    slots: [List; 256],
    occupied: Bitset,
    current: u8,
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Stack {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stack")
            .field("occupied", &self.occupied.len())
            .field("current", &self.current)
            .finish()
    }
}

impl Stack {
    pub fn new() -> Self {
        let slots = arr![List::new(Adapter::new()); 256];
        Self {
            slots,
            occupied: Default::default(),
            current: 0,
        }
    }

    pub const fn current(&self) -> u8 {
        self.current
    }

    pub fn is_empty(&self) -> bool {
        self.occupied.is_empty()
    }

    pub fn insert(&mut self, index: u8, entry: ArcEntry) {
        self.occupied.insert(index);
        let list = self.slot_mut(index);
        list.push_back(entry);
    }

    fn skip(&mut self) {
        if let Some(next) = self.occupied.next_occupied(self.current) {
            self.current = next;
        } else {
            self.current = 0;
        }
    }

    pub fn tick(&mut self, can_skip: bool) -> (List, u8) {
        self.current = self.current.wrapping_add(1);
        if can_skip {
            self.skip();
        }
        let slot = self.take();
        (slot, self.current)
    }

    pub fn take(&mut self) -> List {
        let current = self.current;
        self.occupied.remove(current);
        self.slot_mut(current).take()
    }

    fn slot_mut(&mut self, index: u8) -> &mut List {
        unsafe { self.slots.get_unchecked_mut(index as usize) }
    }
}
