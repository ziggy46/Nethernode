//! Fixed-capacity ring buffer for stats history and log backlog.

use std::collections::VecDeque;

/// Keeps the most recent `capacity` items.
#[derive(Debug, Clone)]
pub struct Ring<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T: Clone> Ring<T> {
    /// An empty ring holding at most `capacity` items.
    pub fn new(capacity: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(capacity.min(1024)),
            capacity: capacity.max(1),
        }
    }

    /// Appends, evicting the oldest item when full.
    pub fn push(&mut self, item: T) {
        if self.items.len() == self.capacity {
            self.items.pop_front();
        }
        self.items.push_back(item);
    }

    /// Items, oldest first.
    pub fn snapshot(&self) -> Vec<T> {
        self.items.iter().cloned().collect()
    }

    /// The newest item.
    pub fn last(&self) -> Option<&T> {
        self.items.back()
    }

    /// Number of items held.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_items() {
        let mut ring = Ring::new(3);
        for i in 0..5 {
            ring.push(i);
        }
        assert_eq!(ring.snapshot(), [2, 3, 4]);
        assert_eq!(ring.last(), Some(&4));
        assert_eq!(ring.len(), 3);
    }
}
