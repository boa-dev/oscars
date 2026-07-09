//! Heap serialization for `PoolAllocator4`
//!
//! Format: little-endian integers
//! `[pool_count]` -> per pool: `[id, size, count, live_count]` -> per slot: `[idx, data]`
//! Slot data must not contain raw pointers

use super::{Pool4, PoolAllocError4, PoolAllocator4};
use rust_alloc::vec::Vec;

// errors

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeserializeError {
    UnexpectedEof,
    /// Index out of range
    InvalidIndex,
    InvalidSlotSize,
    AllocError(PoolAllocError4),
}

impl From<PoolAllocError4> for DeserializeError {
    fn from(e: PoolAllocError4) -> Self {
        Self::AllocError(e)
    }
}

// helpers

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_u32(&mut self) -> Result<u32, DeserializeError> {
        let end = self.pos + 4;
        if end > self.data.len() {
            return Err(DeserializeError::UnexpectedEof);
        }
        let bytes: [u8; 4] = self.data[self.pos..end].try_into().unwrap();
        self.pos = end;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], DeserializeError> {
        let end = self.pos + n;
        if end > self.data.len() {
            return Err(DeserializeError::UnexpectedEof);
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
}

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    fn finish(self) -> Vec<u8> {
        self.buf
    }
}

// public API

/// Serializes all live slots
pub fn serialize(allocator: &PoolAllocator4) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u32(allocator.pools.len() as u32);
    for pool in &allocator.pools {
        let live: Vec<u32> = pool.iter_live().collect();
        w.write_u32(pool.pool_id);
        w.write_u32(pool.slot_size as u32);
        w.write_u32(pool.slot_count as u32);
        w.write_u32(live.len() as u32);
        for idx in live {
            w.write_u32(idx);
            // idx is live
            w.write_bytes(unsafe { pool.slot_bytes(idx as usize) });
        }
    }
    w.finish()
}

/// Reconstructs `PoolAllocator4` from `serialize` bytes
pub fn deserialize(bytes: &[u8]) -> Result<PoolAllocator4, DeserializeError> {
    let mut r = Reader::new(bytes);
    let pool_count = r.read_u32()? as usize;
    let mut allocator = PoolAllocator4::new();

    for _ in 0..pool_count {
        let pool_id = r.read_u32()?;
        let slot_size = r.read_u32()? as usize;
        let slot_count = r.read_u32()? as usize;
        let live_count = r.read_u32()? as usize;

        if slot_size == 0 {
            return Err(DeserializeError::InvalidSlotSize);
        }

        // overflow guard
        if slot_count as u64 > super::MAX_SLOT_IDX as u64 {
            return Err(DeserializeError::InvalidIndex);
        }

        // snapshot is corrupt
        if live_count > slot_count {
            return Err(DeserializeError::InvalidIndex);
        }

        let capacity = slot_size * slot_count + slot_count.div_ceil(64) * 8;
        let pool = Pool4::try_init(pool_id, slot_size, capacity)?;

        for _ in 0..live_count {
            let slot_idx = r.read_u32()? as usize;
            let slot_bytes = r.read_bytes(slot_size)?;

            let allocated = pool.alloc_slot().ok_or(DeserializeError::UnexpectedEof)?;
            // slots are in ascending order
            if allocated != slot_idx {
                return Err(DeserializeError::InvalidIndex);
            }
            // slot is freshly allocated
            unsafe {
                core::ptr::copy_nonoverlapping(
                    slot_bytes.as_ptr(),
                    pool.slot_ptr(allocated).as_ptr(),
                    slot_size,
                );
            }
        }

        if pool_id >= allocator.next_pool_id {
            allocator.next_pool_id = pool_id + 1;
        }
        allocator.pools.push(pool);
    }

    Ok(allocator)
}
