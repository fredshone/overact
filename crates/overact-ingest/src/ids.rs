//! Person ids: one contiguous UTF-8 buffer plus offsets (the format's
//! person-id blobs), indexed by an open-addressing hash table of `u32`s so
//! 10M ids cost about 16 bytes each plus the text.

const EMPTY: u64 = u64::MAX;

/// A table slot: the id's index in the low half and the top 32 bits of its
/// hash in the high half, so most probe collisions are rejected without
/// touching the id bytes.
#[inline]
fn entry(i: u32, h: u64) -> u64 {
    (h & 0xffff_ffff_0000_0000) | i as u64
}

/// A fast, platform-independent 64-bit hash of a byte string.
#[inline]
pub fn hash(bytes: &[u8], seed: u64) -> u64 {
    const K: u64 = 0x51_7c_c1_b7_27_22_0a_95;
    let mut h = seed ^ (bytes.len() as u64).wrapping_mul(K);
    let (chunks, rest) = bytes.as_chunks::<8>();
    for c in chunks {
        h = (h.rotate_left(5) ^ u64::from_le_bytes(*c)).wrapping_mul(K);
    }
    if !rest.is_empty() {
        let mut b = [0u8; 8];
        b[..rest.len()].copy_from_slice(rest);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(b)).wrapping_mul(K);
    }
    // splitmix64 finaliser so every bit depends on every input bit.
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

#[derive(Default)]
pub struct Ids {
    pub data: Vec<u8>,
    /// `offsets[i]..offsets[i+1]` is id `i`; starts with 0.
    pub offsets: Vec<u32>,
    table: Vec<u64>,
}

impl Ids {
    pub fn new() -> Ids {
        Ids::with_capacity(0)
    }

    /// An empty table sized for `n` ids without rehashing.
    pub fn with_capacity(n: usize) -> Ids {
        let mut offsets = Vec::with_capacity(n + 1);
        offsets.push(0);
        Ids {
            data: Vec::new(),
            offsets,
            table: vec![EMPTY; (n * 2 + 1).next_power_of_two().max(1024)],
        }
    }

    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn get(&self, i: usize) -> &[u8] {
        &self.data[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }

    #[inline]
    fn slot(&self, id: &[u8], h: u64) -> (usize, Option<u32>) {
        let mask = self.table.len() - 1;
        let mut s = h as usize & mask;
        loop {
            let v = self.table[s];
            if v == EMPTY {
                return (s, None);
            }
            if (v ^ h) >> 32 == 0 && self.get(v as u32 as usize) == id {
                return (s, Some(v as u32));
            }
            s = (s + 1) & mask;
        }
    }

    #[inline]
    pub fn find(&self, id: &[u8]) -> Option<u32> {
        self.slot(id, hash(id, 0)).1
    }

    /// Index of `id`, inserting it if new. Returns `(index, inserted)`.
    #[inline]
    pub fn insert(&mut self, id: &[u8]) -> crate::Result<(u32, bool)> {
        let h = hash(id, 0);
        let (s, found) = self.slot(id, h);
        if let Some(v) = found {
            return Ok((v, false));
        }
        let i = self.len() as u32;
        if self.data.len() + id.len() > u32::MAX as usize || i == u32::MAX - 1 {
            return Err(crate::Error::new("person ids take more than 4 GB"));
        }
        self.data.extend_from_slice(id);
        self.offsets.push(self.data.len() as u32);
        self.table[s] = entry(i, h);
        if self.len() * 2 > self.table.len() {
            self.rehash(self.table.len() * 2);
        }
        Ok((i, true))
    }

    fn rehash(&mut self, size: usize) {
        self.table = vec![EMPTY; size];
        let mask = size - 1;
        for i in 0..self.len() {
            let h = hash(self.get(i), 0);
            let mut s = h as usize & mask;
            while self.table[s] != EMPTY {
                s = (s + 1) & mask;
            }
            self.table[s] = entry(i as u32, h);
        }
    }

    /// Grows the table (once) to hold `n` ids without further rehashing,
    /// and the text buffer for ids of the average length so far.
    pub fn reserve(&mut self, n: usize) {
        let size = (n * 2 + 1).next_power_of_two();
        if size > self.table.len() {
            let more = n.saturating_sub(self.len());
            self.offsets.reserve(more);
            self.data
                .reserve(more * self.data.len() / self.len().max(1));
            self.rehash(size);
        }
    }

    /// Frees the hash table (lookups no longer work).
    pub fn drop_table(&mut self) {
        self.table = Vec::new();
    }

    /// Keeps only the first `n` ids.
    pub fn truncate(&mut self, n: usize) {
        self.offsets.truncate(n + 1);
        self.data.truncate(self.offsets[n] as usize);
        let size = (n * 2).next_power_of_two().max(1024);
        self.rehash(size);
    }

    /// Keeps the ids where `keep` is true, in order. Returns each old index's
    /// new index (`u32::MAX` if dropped).
    pub fn retain(&mut self, keep: &[bool]) -> Vec<u32> {
        let n = self.len();
        let mut map = vec![u32::MAX; n];
        let (mut w, mut pos, mut a) = (0usize, 0usize, 0usize);
        for i in 0..n {
            // Read the old end before `offsets[w]` (w <= i + 1) is overwritten.
            let b = self.offsets[i + 1] as usize;
            if keep[i] {
                self.data.copy_within(a..b, pos);
                pos += b - a;
                map[i] = w as u32;
                w += 1;
                self.offsets[w] = pos as u32;
            }
            a = b;
        }
        self.offsets.truncate(w + 1);
        self.data.truncate(pos);
        self.rehash((w * 2).next_power_of_two().max(1024));
        map
    }

    /// Reorders ids: new id `k` is old id `order[k]`. Drops the hash table.
    pub fn permute(&mut self, order: &[u32]) {
        self.table = Vec::new();
        if order.iter().enumerate().all(|(k, &o)| o as usize == k) {
            return;
        }
        let mut data = Vec::with_capacity(self.data.len());
        let mut offsets = Vec::with_capacity(order.len() + 1);
        offsets.push(0);
        for &o in order {
            data.extend_from_slice(self.get(o as usize));
            offsets.push(data.len() as u32);
        }
        self.data = data;
        self.offsets = offsets;
        self.table = Vec::new();
    }

    /// Frees the hash table (lookups no longer work).
    pub fn into_parts(self) -> (Vec<u32>, Vec<u8>) {
        (self.offsets, self.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_find_truncate() {
        let mut ids = Ids::new();
        for i in 0..5000 {
            assert_eq!(ids.insert(format!("p{i}").as_bytes()).unwrap(), (i, true));
        }
        assert_eq!(ids.insert(b"p17").unwrap(), (17, false));
        assert_eq!(ids.find(b"p4999"), Some(4999));
        assert_eq!(ids.find(b"nope"), None);
        ids.truncate(10);
        assert_eq!(ids.find(b"p11"), None);
        assert_eq!(ids.find(b"p9"), Some(9));
        assert_eq!(ids.insert(b"x").unwrap(), (10, true));
    }

    #[test]
    fn retain_compacts() {
        let mut ids = Ids::new();
        for id in ["a", "bb", "", "ccc", "d"] {
            ids.insert(id.as_bytes()).unwrap();
        }
        let map = ids.retain(&[false, true, true, false, true]);
        assert_eq!(map, [u32::MAX, 0, 1, u32::MAX, 2]);
        assert_eq!(ids.len(), 3);
        assert_eq!(
            [ids.get(0), ids.get(1), ids.get(2)],
            [&b"bb"[..], b"", b"d"]
        );
        assert_eq!(ids.find(b"d"), Some(2));
        assert_eq!(ids.find(b"a"), None);
        assert_eq!(ids.insert(b"e").unwrap(), (3, true));
    }
}
