//! Slot bookkeeping for the renderer's persistent upload buffers (tasklens-x12tr.1).
//!
//! The renderer keeps a few `MAP_WRITE | COPY_SRC` buffers and reuses them across
//! frames, so a frame's instance and atlas uploads need no new GPU memory. This
//! module only decides which slot a frame uses. It holds no wgpu types, so its
//! logic is tested without a GPU. The renderer owns one buffer per slot, at the
//! same index.

/// The smallest slot buffer, in bytes. A normal frame fits in one slot this size.
pub(crate) const MIN_SLOT_CAPACITY: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SlotMeta {
    capacity: u64,
    /// True from submit until the GPU is done with the slot and it is mapped again.
    in_flight: bool,
}

/// What the renderer must do with its buffers to serve one `acquire`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotPlan {
    /// Use the buffer of this slot as it is.
    Reuse(usize),
    /// The free buffer of this slot is too small: drop it and make one of `capacity` bytes.
    Replace { index: usize, capacity: u64 },
    /// Every slot is in flight: add a buffer of `capacity` bytes as a new last slot.
    Push { capacity: u64 },
}

#[derive(Debug, Default)]
pub(crate) struct StagingRing {
    slots: Vec<SlotMeta>,
    /// Where the next search starts, so slots are used in turn.
    cursor: usize,
    /// Number of slot buffers made so far (every `Replace` and `Push`).
    allocations: u64,
}

impl StagingRing {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Picks a slot for `bytes` bytes of uploads and records the plan in the slot
    /// table. The caller then makes the buffer the plan asks for, writes it, and
    /// calls [`Self::mark_in_flight`] after the submit.
    pub(crate) fn acquire(&mut self, bytes: u64) -> SlotPlan {
        let len = self.slots.len();
        let order = (0..len).map(|step| (self.cursor + step) % len);

        let fitting = order
            .clone()
            .find(|&index| !self.slots[index].in_flight && self.slots[index].capacity >= bytes);
        if let Some(index) = fitting {
            self.cursor = (index + 1) % len;
            return SlotPlan::Reuse(index);
        }

        let capacity = Self::capacity_for(bytes);
        self.allocations += 1;
        let free = order.clone().find(|&index| !self.slots[index].in_flight);
        if let Some(index) = free {
            self.slots[index].capacity = capacity;
            self.cursor = (index + 1) % len;
            return SlotPlan::Replace { index, capacity };
        }

        self.slots.push(SlotMeta {
            capacity,
            in_flight: false,
        });
        self.cursor = 0;
        SlotPlan::Push { capacity }
    }

    /// Size of a new slot buffer for `bytes` bytes: the next power of two, at least
    /// [`MIN_SLOT_CAPACITY`].
    pub(crate) fn capacity_for(bytes: u64) -> u64 {
        bytes.next_power_of_two().max(MIN_SLOT_CAPACITY)
    }

    pub(crate) fn mark_in_flight(&mut self, index: usize) {
        self.slots[index].in_flight = true;
    }

    pub(crate) fn mark_free(&mut self, index: usize) {
        self.slots[index].in_flight = false;
    }

    pub(crate) fn is_in_flight(&self, index: usize) -> bool {
        self.slots[index].in_flight
    }

    pub(crate) fn any_in_flight(&self) -> bool {
        self.slots.iter().any(|slot| slot.in_flight)
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    #[cfg(test)]
    pub(crate) fn allocations(&self) -> u64 {
        self.allocations
    }
}

impl SlotPlan {
    /// The slot index the plan uses, given the slot count after `acquire`.
    pub(crate) fn index(self, slot_count: usize) -> usize {
        match self {
            SlotPlan::Reuse(index) | SlotPlan::Replace { index, .. } => index,
            SlotPlan::Push { .. } => slot_count - 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn reuses_free_slot() {
        let mut ring = StagingRing::new();
        assert_eq!(ring.acquire(1000), SlotPlan::Push { capacity: 4 * MB });
        assert_eq!(ring.acquire(2000), SlotPlan::Reuse(0));
        assert_eq!(ring.acquire(4 * MB), SlotPlan::Reuse(0));
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.allocations(), 1);
    }

    #[test]
    fn pushes_when_all_in_flight() {
        let mut ring = StagingRing::new();
        assert_eq!(ring.acquire(1000), SlotPlan::Push { capacity: 4 * MB });
        ring.mark_in_flight(0);
        assert_eq!(ring.acquire(1000), SlotPlan::Push { capacity: 4 * MB });
        ring.mark_in_flight(1);
        let plan = ring.acquire(5 * MB);
        assert_eq!(plan, SlotPlan::Push { capacity: 8 * MB });
        assert_eq!(plan.index(ring.len()), 2);
        assert_eq!(ring.allocations(), 3);
    }

    #[test]
    fn cursor_wraps_around() {
        let mut ring = StagingRing::new();
        for _ in 0..3 {
            let plan = ring.acquire(1000);
            ring.mark_in_flight(plan.index(ring.len()));
        }
        for index in 0..3 {
            ring.mark_free(index);
        }
        // The last push reset the cursor to slot 0; then the slots come in turn
        // and the search wraps from the last slot back to the first.
        let mut used = Vec::new();
        for _ in 0..7 {
            let plan = ring.acquire(1000);
            used.push(plan.index(ring.len()));
        }
        assert_eq!(used, [0, 1, 2, 0, 1, 2, 0]);

        // From slot 1, with slot 2 in flight, the search wraps to slot 0.
        ring.mark_in_flight(2);
        assert_eq!(ring.acquire(1000), SlotPlan::Reuse(1));
        assert_eq!(ring.acquire(1000), SlotPlan::Reuse(0));
        assert_eq!(ring.allocations(), 3);
    }

    #[test]
    fn replaces_too_small_slot() {
        let mut ring = StagingRing::new();
        assert_eq!(ring.acquire(1000), SlotPlan::Push { capacity: 4 * MB });
        ring.mark_in_flight(0);
        assert_eq!(ring.acquire(1000), SlotPlan::Push { capacity: 4 * MB });
        ring.mark_free(0);

        // Both slots are free and too small: the search from the cursor picks one.
        let plan = ring.acquire(9 * MB);
        assert_eq!(
            plan,
            SlotPlan::Replace {
                index: 0,
                capacity: 16 * MB
            }
        );
        assert_eq!(ring.allocations(), 3);

        // A free slot that fits wins over a too-small one nearer the cursor.
        assert_eq!(ring.acquire(9 * MB), SlotPlan::Reuse(0));
        assert_eq!(ring.allocations(), 3);
    }

    #[test]
    fn steady_three_in_flight_allocates_three_times() {
        let mut ring = StagingRing::new();
        let mut in_flight = VecDeque::new();
        for frame in 0..600_u64 {
            // The GPU finishes a frame two frames later, so up to three slots are
            // in flight at once, the new one included.
            if in_flight.len() == 3 {
                let done = in_flight.pop_front().expect("three slots in flight");
                ring.mark_free(done);
            }
            let bytes = 1_000_000 + (frame % 7) * 300_000;
            let plan = ring.acquire(bytes);
            let index = plan.index(ring.len());
            assert!(!ring.is_in_flight(index), "frame {frame} got a busy slot");
            ring.mark_in_flight(index);
            in_flight.push_back(index);
        }
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.allocations(), 3);
        assert!(ring.any_in_flight());
    }
}
