//! SDK-free mirror of the 11 `VectorDiff` variants.

/// A change to an ordered list, mirroring the SDK's vector diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListDiff<T> {
    /// Add items at the end.
    Append {
        /// Items to add.
        values: Vec<T>,
    },
    /// Remove everything.
    Clear,
    /// Add an item at the front.
    PushFront {
        /// Item to add.
        value: T,
    },
    /// Add an item at the end.
    PushBack {
        /// Item to add.
        value: T,
    },
    /// Remove the first item.
    PopFront,
    /// Remove the last item.
    PopBack,
    /// Insert an item at `index`.
    Insert {
        /// Position.
        index: usize,
        /// Item to insert.
        value: T,
    },
    /// Replace the item at `index`.
    Set {
        /// Position.
        index: usize,
        /// New item.
        value: T,
    },
    /// Remove the item at `index`.
    Remove {
        /// Position.
        index: usize,
    },
    /// Keep only the first `length` items.
    Truncate {
        /// New length.
        length: usize,
    },
    /// Replace the whole list.
    Reset {
        /// New contents.
        values: Vec<T>,
    },
}

impl<T: Clone> ListDiff<T> {
    /// Applies the diff to `list`.
    ///
    /// A diff whose index is out of range is ignored instead of panicking, so
    /// a malformed stream cannot crash a consumer.
    ///
    /// ```
    /// use tessera_core::api::ListDiff;
    /// let mut v = vec![1, 2];
    /// ListDiff::PushBack { value: 3 }.apply(&mut v);
    /// assert_eq!(v, [1, 2, 3]);
    /// ```
    pub fn apply(&self, list: &mut Vec<T>) {
        match self {
            Self::Append { values } => list.extend(values.iter().cloned()),
            Self::Clear => list.clear(),
            Self::PushFront { value } => list.insert(0, value.clone()),
            Self::PushBack { value } => list.push(value.clone()),
            Self::PopFront => {
                if !list.is_empty() {
                    list.remove(0);
                }
            }
            Self::PopBack => {
                list.pop();
            }
            Self::Insert { index, value } => {
                if *index <= list.len() {
                    list.insert(*index, value.clone());
                }
            }
            Self::Set { index, value } => {
                if let Some(slot) = list.get_mut(*index) {
                    *slot = value.clone();
                }
            }
            Self::Remove { index } => {
                if *index < list.len() {
                    list.remove(*index);
                }
            }
            Self::Truncate { length } => list.truncate(*length),
            Self::Reset { values } => {
                list.clear();
                list.extend(values.iter().cloned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ListDiff::*;

    fn run(start: &[i32], diff: super::ListDiff<i32>) -> Vec<i32> {
        let mut v = start.to_vec();
        diff.apply(&mut v);
        v
    }

    #[test]
    fn append() {
        assert_eq!(run(&[1], Append { values: vec![2, 3] }), [1, 2, 3]);
    }
    #[test]
    fn clear() {
        assert_eq!(run(&[1, 2], Clear), Vec::<i32>::new());
    }
    #[test]
    fn push_front() {
        assert_eq!(run(&[2, 3], PushFront { value: 1 }), [1, 2, 3]);
    }
    #[test]
    fn push_back() {
        assert_eq!(run(&[1, 2], PushBack { value: 3 }), [1, 2, 3]);
    }
    #[test]
    fn pop_front() {
        assert_eq!(run(&[1, 2, 3], PopFront), [2, 3]);
    }
    #[test]
    fn pop_back() {
        assert_eq!(run(&[1, 2, 3], PopBack), [1, 2]);
    }
    #[test]
    fn insert() {
        assert_eq!(run(&[1, 3], Insert { index: 1, value: 2 }), [1, 2, 3]);
    }
    #[test]
    fn set() {
        assert_eq!(run(&[1, 9, 3], Set { index: 1, value: 2 }), [1, 2, 3]);
    }
    #[test]
    fn remove() {
        assert_eq!(run(&[1, 9, 2], Remove { index: 1 }), [1, 2]);
    }
    #[test]
    fn truncate() {
        assert_eq!(run(&[1, 2, 3], Truncate { length: 2 }), [1, 2]);
    }
    #[test]
    fn reset() {
        assert_eq!(
            run(
                &[1, 2],
                Reset {
                    values: vec![7, 8, 9]
                }
            ),
            [7, 8, 9]
        );
    }
    #[test]
    fn out_of_range_is_ignored() {
        assert_eq!(run(&[1], Set { index: 5, value: 2 }), [1]);
        assert_eq!(run(&[1], Remove { index: 5 }), [1]);
        assert_eq!(run(&[1], Insert { index: 5, value: 2 }), [1]);
        assert_eq!(run(&[], PopFront), Vec::<i32>::new());
    }
}
