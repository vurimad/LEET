//! Resource usage flags.

use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ResourceUsage {
    bits: u8,
}

impl ResourceUsage {
    pub const NONE: Self = Self { bits: 0 };
    pub const READ: Self = Self { bits: 1 << 0 };
    pub const WRITE: Self = Self { bits: 1 << 1 };
    pub const NO_DISCARD: Self = Self { bits: 1 << 2 };

    const KNOWN_BITS: u8 = Self::READ.bits | Self::WRITE.bits | Self::NO_DISCARD.bits;

    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::KNOWN_BITS == 0 {
            Some(Self { bits })
        } else {
            None
        }
    }

    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self {
            bits: bits & Self::KNOWN_BITS,
        }
    }

    pub const fn bits(self) -> u8 {
        self.bits
    }

    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.bits & other.bits != 0
    }
}

impl BitOr for ResourceUsage {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self {
            bits: self.bits | rhs.bits,
        }
    }
}

impl BitOrAssign for ResourceUsage {
    fn bitor_assign(&mut self, rhs: Self) {
        self.bits |= rhs.bits;
    }
}

impl BitAnd for ResourceUsage {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self {
            bits: self.bits & rhs.bits,
        }
    }
}

impl BitAndAssign for ResourceUsage {
    fn bitand_assign(&mut self, rhs: Self) {
        self.bits &= rhs.bits;
    }
}
