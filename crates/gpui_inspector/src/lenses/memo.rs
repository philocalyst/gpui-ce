//! Memoized derived data: recomputed only when its key changes.
//!
//! Lenses derive heavy data from the capture (percentiles, flame layouts,
//! bottom-up tables, audits). Rendering must never recompute it, so each
//! derivation lives in a [`Memo`] keyed by what it depends on, typically
//! `(generation, selection…)`.

use std::rc::Rc;

/// One derived value and the key it was computed for.
pub(crate) struct Memo<K, V> {
    entry: Option<(K, Rc<V>)>,
}

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self { entry: None }
    }
}

impl<K: PartialEq, V> Memo<K, V> {
    /// The value for `key`, computing it only if the key changed since the
    /// last call.
    pub fn get(&mut self, key: K, compute: impl FnOnce() -> V) -> Rc<V> {
        match &self.entry {
            Some((cached, value)) if *cached == key => value.clone(),
            _ => {
                let value = Rc::new(compute());
                self.entry = Some((key, value.clone()));
                value
            }
        }
    }

    /// Whether the value for `key` is already computed.
    #[cfg(test)]
    pub fn is_fresh(&self, key: &K) -> bool {
        self.entry.as_ref().is_some_and(|(cached, _)| cached == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn recomputes_only_when_the_key_changes() {
        let runs = Cell::new(0);
        let mut memo = Memo::default();
        let compute = |value: u32| {
            runs.set(runs.get() + 1);
            value * 10
        };
        assert_eq!(*memo.get((1, 7), || compute(1)), 10);
        assert_eq!(*memo.get((1, 7), || compute(2)), 10, "same key: cached");
        assert!(memo.is_fresh(&(1, 7)));
        assert_eq!(*memo.get((2, 7), || compute(3)), 30, "new generation");
        assert_eq!(*memo.get((2, 8), || compute(4)), 40, "new selection");
        assert_eq!(runs.get(), 3);
        assert!(!memo.is_fresh(&(1, 7)));
    }
}
