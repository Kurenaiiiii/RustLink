use std::collections::VecDeque;

/// Ring-buffer queue matching NodeLink's headQueue.ts semantics.
/// Uses VecDeque internally for O(1) enqueue/dequeue.
pub struct HeadQueue<T> {
    items: VecDeque<T>,
}

impl<T> HeadQueue<T> {
    pub fn new() -> Self {
        Self {
            items: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn enqueue(&mut self, item: T) {
        self.items.push_back(item);
    }

    pub fn dequeue(&mut self) -> Option<T> {
        self.items.pop_front()
    }
}

impl<T> Default for HeadQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}
