use super::{
    entry::{ArcEntry, List},
    stack::Stack,
};

#[derive(Debug, Default)]
pub struct Wheel {
    stacks: [Stack; 8],
    pending_wake: List,
}

macro_rules! stack_map {
    ($stack:expr, $cb:expr) => {{
        let cb = $cb;
        [
            cb(&$stack[0]),
            cb(&$stack[1]),
            cb(&$stack[2]),
            cb(&$stack[3]),
            cb(&$stack[4]),
            cb(&$stack[5]),
            cb(&$stack[6]),
            cb(&$stack[7]),
        ]
    }};
}

impl Wheel {
    pub fn ticks(&self) -> u64 {
        u64::from_le_bytes(stack_map!(self.stacks, |stack: &Stack| stack.current()))
    }

    pub fn is_empty(&self) -> bool {
        macro_rules! is_empty {
            () => {
                is_empty!([0, 1, 2, 3, 4, 5, 6, 7])
            };
            ([$($idx:expr),*]) => {
                $(self.stacks[$idx].is_empty())&*
            };
        }
        is_empty!()
    }

    pub fn insert(&mut self, entry: ArcEntry) {
        let ticks = self.ticks();
        entry.set_start_tick(ticks);
        self.insert_at(entry, ticks);
    }

    fn insert_at(&mut self, entry: ArcEntry, start_tick: u64) -> Option<u64> {
        let delay = entry.delay();
        let absolute_time = delay.wrapping_add(start_tick);
        let now = self.ticks();
        let zero_time = (absolute_time ^ now).to_be();

        if zero_time == 0 {
            self.pending_wake.push_back(entry);
            return Some(delay);
        }

        let absolute_bytes = absolute_time.to_le_bytes();
        let leading = zero_time.leading_zeros();

        let index = (leading / 8) as usize;
        let position = absolute_bytes[index];

        self.stack_mut(index).insert(position, entry);

        None
    }

    pub fn advance(&mut self) -> Option<u64> {
        let start = self.ticks();
        let has_pending = !self.pending_wake.is_empty();

        if has_pending {
            return Some(0);
        }

        if self.is_empty() {
            return None;
        }

        loop {
            let elapsed = self.advance_once(start)?;
            if elapsed != u64::MAX {
                return Some(self.ticks() - start);
            }
        }
    }

    fn advance_once(&mut self, start: u64) -> Option<u64> {
        let mut can_skip = true;
        let mut is_empty = true;
        let mut min_elapsed = u64::MAX;

        for index in 0..self.stacks.len() {
            let stack = self.stack_mut(index);

            let (list, next) = stack.tick(can_skip);

            // children can only skip if this is also empty
            can_skip &= stack.is_empty();
            is_empty &= can_skip;

            for entry in list {
                let start_tick = entry.start_tick();
                if let Some(delay) = self.insert_at(entry, start_tick) {
                    let elapsed = if let Some(elapsed) = delay.checked_sub(start - start_tick) {
                        elapsed
                    } else {
                        start_tick + delay
                    };
                    min_elapsed = min_elapsed.min(elapsed);
                }
                is_empty = false;
            }

            // we need to start at the top again
            if next != 0 || min_elapsed != u64::MAX {
                return Some(min_elapsed);
            }
        }

        if is_empty {
            return None;
        }

        Some(min_elapsed)
    }

    pub fn wake(&mut self) -> usize {
        let mut count = 0;

        for entry in self.pending_wake.take() {
            count += 1;
            if entry.wake() {
                // reinsert entries if they are periodic
                self.insert(entry);
            }
        }

        count
    }

    fn stack_mut(&mut self, index: usize) -> &mut Stack {
        if cfg!(test) {
            debug_assert!(index < self.stacks.len());
        }
        unsafe { self.stacks.get_unchecked_mut(index) }
    }
}

#[cfg(test)]
mod tests {
    use super::{super::entry::Entry, *};
    use alloc::{vec, vec::Vec};
    use bolero::{check, generator::*};
    use core::time::Duration;

    #[test]
    fn insert_advance_wake_check() {
        let max_ticks = Duration::from_secs(3600).as_nanos() as u64;

        let entry = gen::<Vec<u64>>().with().values(0..max_ticks);
        let entries = gen::<Vec<_>>().with().values(entry);

        check!()
            .with_generator(entries)
            .for_each(|entries| test_helper(&entries[..]));
    }

    fn test_helper<T: AsRef<[u64]>>(entries: &[T]) {
        let mut wheel = Wheel::default();
        let mut sorted = vec![];

        let mut total_ticks = 0;

        for entries in entries.iter().map(AsRef::as_ref) {
            sorted.extend_from_slice(entries);
            sorted.sort_unstable();

            let mut should_wake = false;
            for entry in entries.iter().copied() {
                // adding a 0-tick will immediately wake the entry
                should_wake |= entry == 0;
                wheel.insert(Entry::new(entry, false));
            }

            let mut sorted = sorted.drain(..);

            let woken = wheel.wake();

            assert_eq!(woken > 0, should_wake);

            for _ in 0..woken {
                sorted.next();
            }

            let mut elapsed = 0;

            while let Some(expected) = sorted.next() {
                std::dbg!(&wheel);
                let delta = expected - elapsed;
                assert_eq!(wheel.advance(), Some(delta));
                elapsed += delta;

                assert_eq!(
                    wheel.advance(),
                    Some(0),
                    "the wheel should not advance while there are pending items"
                );

                for _ in (0..wheel.wake()).skip(1) {
                    assert_eq!(
                        sorted.next(),
                        Some(expected),
                        "any additional items should be equal"
                    );
                }
            }

            assert!(wheel.is_empty());
            assert_eq!(wheel.advance(), None);
            assert_eq!(wheel.wake(), 0);
            assert!(wheel.is_empty());

            total_ticks += elapsed;

            assert_eq!(wheel.ticks(), total_ticks);
        }
    }

    #[test]
    fn empty_test() {
        let mut wheel = Wheel::default();
        assert_eq!(wheel.ticks(), 0);
        assert!(wheel.is_empty());
        assert_eq!(wheel.advance(), None);
        assert_eq!(wheel.wake(), 0);
    }

    #[test]
    fn crossing_test() {
        for t in [250..260, 510..520, 65790..65800].iter().cloned().flatten() {
            test_helper(&[[t, t + 1]]);
        }
    }

    #[test]
    fn duplicate_test() {
        test_helper(&[&[1, 489][..], &[24, 279][..]]);
    }

    #[test]
    fn overflow_test() {
        test_helper(&[
            &[3588254211306][..],
            &[799215800378, 10940666347][..],
            &[][..],
        ]);
    }
}
