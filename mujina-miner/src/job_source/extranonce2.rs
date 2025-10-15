//! Extranonce2 types for Bitcoin mining.
//!
//! In Bitcoin mining, the extranonce2 field is the miner's primary mechanism for
//! generating unique work across the search space. This module provides two types:
//!
//! - `Extranonce2`: An immutable value with a specific size (1-8 bytes)
//! - `Extranonce2Template`: A mutable range generator that produces `Extranonce2` values
//!
//! Mining pools allocate a specific byte size for extranonce2 (typically 4-8 bytes),
//! which determines how many unique coinbase transactions a miner can generate before
//! needing new work. The template type provides range splitting for dividing work
//! between multiple domains, while the value type represents specific extranonce2
//! values used in share submissions and coinbase construction.

use std::fmt;

use thiserror::Error;

/// Errors that can occur when creating Extranonce2 types.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum Extranonce2Error {
    #[error("Invalid extranonce2 size: {0} (must be 1-8 bytes)")]
    InvalidSize(u8),

    #[error("Value {0} exceeds maximum for size {1} bytes")]
    ValueTooLarge(u64, u8),

    #[error("Invalid range: min {0} >= max {1}")]
    InvalidRange(u64, u64),
}

/// A specific extranonce2 value with fixed size.
///
/// This is an immutable value type representing a single extranonce2 that will be
/// serialized into a coinbase transaction or stored in a share. The value is stored
/// as a u64 but serializes to the specified number of bytes (1-8).
///
/// Use `Extranonce2Template` to generate sequences of these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Extranonce2 {
    value: u64,
    size: u8,
}

impl Extranonce2 {
    /// Create a new extranonce2 value.
    ///
    /// Returns an error if the size is invalid (must be 1-8 bytes) or if the value
    /// is too large to fit in the specified size.
    pub fn new(value: u64, size: u8) -> Result<Self, Extranonce2Error> {
        if size == 0 || size > 8 {
            return Err(Extranonce2Error::InvalidSize(size));
        }

        let max = Self::max_for_size(size);
        if value > max {
            return Err(Extranonce2Error::ValueTooLarge(value, size));
        }

        Ok(Self { value, size })
    }

    /// Get the value as a u64.
    pub fn value(&self) -> u64 {
        self.value
    }

    /// Get the size in bytes.
    pub fn size(&self) -> u8 {
        self.size
    }

    /// Get the maximum value for a given size.
    fn max_for_size(size: u8) -> u64 {
        if size >= 8 {
            u64::MAX
        } else {
            (1u64 << (size * 8)) - 1
        }
    }

    /// Extend a vector with the serialized bytes of this extranonce2.
    pub fn extend_vec(&self, vec: &mut Vec<u8>) {
        vec.extend_from_slice(&self.value.to_le_bytes()[..self.size as usize]);
    }
}

impl From<Extranonce2> for Vec<u8> {
    /// Convert to little-endian bytes for inclusion in coinbase transaction.
    fn from(ext: Extranonce2) -> Vec<u8> {
        ext.value.to_le_bytes()[..ext.size as usize].to_vec()
    }
}

impl fmt::Display for Extranonce2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:0width$x}", self.value, width = self.size as usize * 2)
    }
}

/// A template for generating extranonce2 values within a specified range.
///
/// This is a mutable generator type that tracks a current position within a range
/// [min, max] and produces `Extranonce2` values. It's used for dividing work between
/// domains and tracking progress through assigned extranonce2 space.
///
/// The template can be split into non-overlapping sub-ranges for distributing work
/// to multiple boards or chip chains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extranonce2Template {
    min: u64,
    max: u64,
    current: u64,
    size: u8,
}

impl Extranonce2Template {
    /// Create a new template covering the full range for the given size.
    ///
    /// Creates a template with min=0, max=maximum value for size, current=0.
    pub fn new(size: u8) -> Result<Self, Extranonce2Error> {
        if size == 0 || size > 8 {
            return Err(Extranonce2Error::InvalidSize(size));
        }

        let max = Extranonce2::max_for_size(size);
        Ok(Self {
            min: 0,
            max,
            current: 0,
            size,
        })
    }

    /// Create a new template with a custom range.
    pub fn new_range(min: u64, max: u64, size: u8) -> Result<Self, Extranonce2Error> {
        if size == 0 || size > 8 {
            return Err(Extranonce2Error::InvalidSize(size));
        }

        if min >= max {
            return Err(Extranonce2Error::InvalidRange(min, max));
        }

        let size_max = Extranonce2::max_for_size(size);
        if max > size_max {
            return Err(Extranonce2Error::ValueTooLarge(max, size));
        }

        Ok(Self {
            min,
            max,
            current: min,
            size,
        })
    }

    /// Get the current value as an `Extranonce2`.
    pub fn current(&self) -> Extranonce2 {
        // SAFETY: current is always valid because we maintain the invariant
        // that min <= current <= max, and max is validated against size
        Extranonce2::new(self.current, self.size).expect("current value should always be valid")
    }

    /// Increment to the next value and return it.
    ///
    /// Returns `None` if the range is exhausted (current would exceed max).
    pub fn next(&mut self) -> Option<Extranonce2> {
        if self.current >= self.max {
            return None;
        }
        self.current += 1;
        Some(self.current())
    }

    /// Increment the current value.
    ///
    /// Returns `false` if the range is exhausted (reached max), `true` otherwise.
    pub fn increment(&mut self) -> bool {
        if self.current >= self.max {
            false
        } else {
            self.current += 1;
            true
        }
    }

    /// Reset to the minimum value.
    pub fn reset(&mut self) {
        self.current = self.min;
    }

    /// Get the total search space (number of values in the range).
    pub fn search_space(&self) -> u64 {
        self.max - self.min + 1
    }

    /// Split this range into `n` non-overlapping sub-ranges.
    ///
    /// Useful for dividing work between multiple boards. Each sub-range will have
    /// approximately the same size, with any remainder distributed among the first
    /// few ranges.
    ///
    /// Returns `None` if `n` is 0 or if the range is too small to split.
    pub fn split(&self, n: usize) -> Option<Vec<Extranonce2Template>> {
        if n == 0 {
            return None;
        }

        if n == 1 {
            return Some(vec![self.clone()]);
        }

        let total = self.search_space();
        if (total as usize) < n {
            return None;
        }

        let chunk_size = total / (n as u64);
        let remainder = total % (n as u64);

        let mut ranges = Vec::with_capacity(n);
        let mut start = self.min;

        for i in 0..n {
            // Distribute remainder among first few chunks
            let size = chunk_size + if (i as u64) < remainder { 1 } else { 0 };
            let end = start + size - 1;

            ranges.push(
                Self::new_range(start, end, self.size).expect("sub-range should always be valid"),
            );

            start = end + 1;
        }

        Some(ranges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Extranonce2 value type tests
    #[test]
    fn test_extranonce2_new() {
        let ext = Extranonce2::new(0, 4).unwrap();
        assert_eq!(ext.value(), 0);
        assert_eq!(ext.size(), 4);

        let ext = Extranonce2::new(0x1234, 4).unwrap();
        assert_eq!(ext.value(), 0x1234);
    }

    #[test]
    fn test_extranonce2_errors() {
        // Invalid size
        assert!(matches!(
            Extranonce2::new(0, 0),
            Err(Extranonce2Error::InvalidSize(0))
        ));
        assert!(matches!(
            Extranonce2::new(0, 9),
            Err(Extranonce2Error::InvalidSize(9))
        ));

        // Value too large for size
        assert!(matches!(
            Extranonce2::new(0x100, 1),
            Err(Extranonce2Error::ValueTooLarge(0x100, 1))
        ));
        assert!(matches!(
            Extranonce2::new(0x1_0000, 2),
            Err(Extranonce2Error::ValueTooLarge(0x1_0000, 2))
        ));
    }

    #[test]
    fn test_extranonce2_to_bytes() {
        let ext = Extranonce2::new(0, 4).unwrap();
        assert_eq!(Vec::<u8>::from(ext), vec![0, 0, 0, 0]);

        let ext = Extranonce2::new(0x1234, 4).unwrap();
        assert_eq!(Vec::<u8>::from(ext), vec![0x34, 0x12, 0, 0]); // Little-endian

        let ext = Extranonce2::new(0xab, 1).unwrap();
        assert_eq!(Vec::<u8>::from(ext), vec![0xab]);
    }

    #[test]
    fn test_extranonce2_display() {
        let ext = Extranonce2::new(0, 4).unwrap();
        assert_eq!(format!("{}", ext), "00000000");

        let ext = Extranonce2::new(0x1234, 4).unwrap();
        assert_eq!(format!("{}", ext), "00001234");

        let ext = Extranonce2::new(0xab, 2).unwrap();
        assert_eq!(format!("{}", ext), "00ab");
    }

    // Extranonce2Template tests
    #[test]
    fn test_template_new() {
        let template = Extranonce2Template::new(4).unwrap();
        assert_eq!(template.search_space(), 1u64 << 32);

        let current = template.current();
        assert_eq!(current.value(), 0);
        assert_eq!(current.size(), 4);
    }

    #[test]
    fn test_template_new_range() {
        let template = Extranonce2Template::new_range(0x1000, 0x2000, 4).unwrap();
        assert_eq!(template.search_space(), 0x1001);

        let current = template.current();
        assert_eq!(current.value(), 0x1000);
    }

    #[test]
    fn test_template_increment() {
        let mut template = Extranonce2Template::new_range(0, 2, 1).unwrap();

        assert_eq!(template.current().value(), 0);
        assert!(template.increment());
        assert_eq!(template.current().value(), 1);
        assert!(template.increment());
        assert_eq!(template.current().value(), 2);
        assert!(!template.increment()); // At max
    }

    #[test]
    fn test_template_next() {
        let mut template = Extranonce2Template::new_range(0, 2, 1).unwrap();

        assert_eq!(template.next().unwrap().value(), 1);
        assert_eq!(template.next().unwrap().value(), 2);
        assert!(template.next().is_none());
    }

    #[test]
    fn test_template_reset() {
        let mut template = Extranonce2Template::new_range(10, 20, 1).unwrap();

        template.increment();
        template.increment();
        assert_eq!(template.current().value(), 12);

        template.reset();
        assert_eq!(template.current().value(), 10);
    }

    #[test]
    fn test_template_split() {
        let template = Extranonce2Template::new_range(0, 99, 1).unwrap();
        let splits = template.split(4).unwrap();

        assert_eq!(splits.len(), 4);
        assert_eq!(splits[0].search_space(), 25);
        assert_eq!(splits[1].search_space(), 25);
        assert_eq!(splits[2].search_space(), 25);
        assert_eq!(splits[3].search_space(), 25);

        // Check boundaries
        assert_eq!(splits[0].min, 0);
        assert_eq!(splits[0].max, 24);
        assert_eq!(splits[1].min, 25);
        assert_eq!(splits[1].max, 49);
        assert_eq!(splits[2].min, 50);
        assert_eq!(splits[2].max, 74);
        assert_eq!(splits[3].min, 75);
        assert_eq!(splits[3].max, 99);
    }

    #[test]
    fn test_template_split_with_remainder() {
        let template = Extranonce2Template::new_range(0, 9, 1).unwrap();
        let splits = template.split(3).unwrap();

        assert_eq!(splits.len(), 3);
        // 10 values split 3 ways: 4, 3, 3
        assert_eq!(splits[0].search_space(), 4);
        assert_eq!(splits[1].search_space(), 3);
        assert_eq!(splits[2].search_space(), 3);
    }
}
