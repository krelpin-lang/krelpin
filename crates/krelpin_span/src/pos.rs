use std::fmt;
use std::ops::{Add, Range, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BytePos(pub u32);

impl Add<u32> for BytePos {
    type Output = BytePos;

    fn add(self, rhs: u32) -> BytePos {
        BytePos(self.0 + rhs)
    }
}

impl Sub<u32> for BytePos {
    type Output = BytePos;

    fn sub(self, rhs: u32) -> BytePos {
        BytePos(self.0 - rhs)
    }
}

impl Sub for BytePos {
    type Output = u32;

    fn sub(self, rhs: BytePos) -> u32 {
        self.0 - rhs.0
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub lo: BytePos,
    pub hi: BytePos,
}

impl Span {
    pub const DUMMY: Span = Span {
        lo: BytePos(0),
        hi: BytePos(0),
    };

    pub fn new(lo: BytePos, hi: BytePos) -> Span {
        if lo <= hi {
            Span { lo, hi }
        } else {
            Span { lo: hi, hi: lo }
        }
    }

    pub fn len(self) -> u32 {
        self.hi - self.lo
    }

    pub fn is_empty(self) -> bool {
        self.lo == self.hi
    }

    pub fn to(self, end: Span) -> Span {
        Span::new(self.lo.min(end.lo), self.hi.max(end.hi))
    }

    pub fn shrink_to_lo(self) -> Span {
        Span::new(self.lo, self.lo)
    }

    pub fn shrink_to_hi(self) -> Span {
        Span::new(self.hi, self.hi)
    }

    pub fn subspan(self, range: Range<usize>) -> Span {
        Span::new(self.lo + range.start as u32, self.lo + range.end as u32)
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.lo.0, self.hi.0)
    }
}
