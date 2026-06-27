//! Custom 32 bit pointer types
//! 
//! [`CustomPtr`] stores `(pool_id, slot_idx)` instead of an address so it survives serialization.
//! [`Gc<'gc, T>`] wraps it with a lifetime brand, use `resolve` instead of `Deref`

use core::marker::PhantomData;
use core::num::NonZeroU32;

const SLOT_BITS: u32 = 20;
const SLOT_MASK: u32 = (1 << SLOT_BITS) - 1;
pub const MAX_POOL_ID: u32 = (1 << (32 - SLOT_BITS)) - 1; // 4095
pub const MAX_SLOT_IDX: u32 = SLOT_MASK;

/// A stable, address independent index into a [`PoolAllocator4`](super::PoolAllocator4)
///
/// `Option<CustomPtr>` is the same size as `CustomPtr` 
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CustomPtr(NonZeroU32);

impl CustomPtr {
    /// Packs `pool_id` and `slot_idx` into a `CustomPtr`
    #[inline]
    pub fn new(pool_id: u32, slot_idx: u32) -> Option<Self> {
        if pool_id > MAX_POOL_ID || slot_idx > MAX_SLOT_IDX {
            return None;
        }
        let packed = (pool_id << SLOT_BITS) | slot_idx;
        // +1 bias keeps NonZeroU32 valid; checked_add handles the overflow
        packed
            .checked_add(1)
            .and_then(NonZeroU32::new)
            .map(CustomPtr)
    }

    /// Pool index (bits 31–20)
    #[inline]
    pub fn pool_id(self) -> usize {
        ((self.0.get() - 1) >> SLOT_BITS) as usize
    }

    /// Slot index (bits 19–0)
    #[inline]
    pub fn slot_idx(self) -> usize {
        ((self.0.get() - 1) & SLOT_MASK) as usize
    }

    /// Raw `u32`
    #[inline]
    pub fn to_raw(self) -> u32 {
        self.0.get()
    }

    /// Reconstruct from `to_raw`
    ///
    /// # Safety
    /// Only pass valid raw values
    #[inline]
    pub fn from_raw(raw: u32) -> Option<Self> {
        NonZeroU32::new(raw).map(CustomPtr)
    }
}

impl core::fmt::Debug for CustomPtr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "CustomPtr({}, {})", self.pool_id(), self.slot_idx())
    }
}

impl core::fmt::Display for CustomPtr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "({}, {})", self.pool_id(), self.slot_idx())
    }
}

/// Lifetime branded GC handle backed by a `CustomPtr`
#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct Gc<'gc, T: ?Sized> {
    // use from_custom_ptr
    pub(super) ptr: CustomPtr,
    pub(super) _marker: PhantomData<(&'gc (), *const T)>,
}

// Gc is a plain integer
unsafe impl<'gc, T: ?Sized> Send for Gc<'gc, T> {}
unsafe impl<'gc, T: ?Sized> Sync for Gc<'gc, T> {}

impl<'gc, T> Gc<'gc, T> {
    /// Returns underlying `CustomPtr`
    #[inline]
    pub fn as_custom_ptr(self) -> CustomPtr {
        self.ptr
    }

    /// Rebuilds `Gc` from a `CustomPtr`
    ///
    /// # Safety
    /// `ptr` must refer to a live slot of type `T`
    #[inline]
    pub unsafe fn from_custom_ptr(ptr: CustomPtr) -> Self {
        Self {
            ptr,
            _marker: PhantomData,
        }
    }
}

impl<'gc, T: ?Sized> core::fmt::Debug for Gc<'gc, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Gc({:?})", self.ptr)
    }
}

#[cfg(test)]
mod ptr_unit_tests {
    use super::*;

    #[test]
    fn roundtrip_pool_and_slot() {
        let cases: &[(u32, u32)] = &[
            (0, 0),
            (0, MAX_SLOT_IDX),
            (MAX_POOL_ID, 0),
            (1, 100),
            (255, 1_000),
        ];
        for &(pool_id, slot_idx) in cases {
            let ptr = CustomPtr::new(pool_id, slot_idx)
                .unwrap_or_else(|| panic!("failed for ({pool_id}, {slot_idx})"));
            assert_eq!(ptr.pool_id() as u32, pool_id);
            assert_eq!(ptr.slot_idx() as u32, slot_idx);
        }
        // +1 overflows to None
        assert!(CustomPtr::new(MAX_POOL_ID, MAX_SLOT_IDX).is_none());
    }

    #[test]
    fn out_of_range_pool_returns_none() {
        assert!(CustomPtr::new(MAX_POOL_ID + 1, 0).is_none());
    }

    #[test]
    fn out_of_range_slot_returns_none() {
        assert!(CustomPtr::new(0, MAX_SLOT_IDX + 1).is_none());
    }

    #[test]
    fn option_size_is_same_as_custom_ptr() {
        assert_eq!(
            core::mem::size_of::<Option<CustomPtr>>(),
            core::mem::size_of::<CustomPtr>(),
        );
    }

    #[test]
    fn raw_roundtrip() {
        let ptr = CustomPtr::new(42, 7).unwrap();
        assert_eq!(ptr, CustomPtr::from_raw(ptr.to_raw()).unwrap());
    }

    #[test]
    fn from_raw_zero_returns_none() {
        assert!(CustomPtr::from_raw(0).is_none());
    }

    fn _assert_send_sync<T: Send + Sync>() {}
    fn _check() {
        _assert_send_sync::<Gc<'static, i32>>();
    }
}

