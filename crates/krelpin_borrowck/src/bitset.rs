#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BitSet {
    words: Vec<u64>,
    len: usize,
}

impl BitSet {
    pub(crate) fn new(len: usize) -> BitSet {
        BitSet {
            words: vec![0; len.div_ceil(64)],
            len,
        }
    }

    pub(crate) fn insert(&mut self, index: usize) {
        self.words[index / 64] |= 1 << (index % 64);
    }

    pub(crate) fn remove(&mut self, index: usize) {
        self.words[index / 64] &= !(1 << (index % 64));
    }

    pub(crate) fn contains(&self, index: usize) -> bool {
        self.words[index / 64] & (1 << (index % 64)) != 0
    }

    pub(crate) fn union(&mut self, other: &BitSet) -> bool {
        let mut changed = false;
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            let merged = *word | other;
            changed |= merged != *word;
            *word = merged;
        }
        changed
    }

    pub(crate) fn clear(&mut self) {
        self.words.fill(0);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len).filter(|&index| self.contains(index))
    }
}
