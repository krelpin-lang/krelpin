use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefId(u32);

impl DefId {
    pub fn from_index(index: usize) -> DefId {
        DefId(u32::try_from(index).expect("too many definitions"))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Debug for DefId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DefId({})", self.0)
    }
}
