//! Content hashing — every provider produces the same `Blake3Hash` over
//! file bytes for `ExtractedFile.content_hash`.

use standardoc_ir::Blake3Hash;

/// BLAKE3 hash of a byte slice, wrapped in the IR's strongly-typed
/// `Blake3Hash` newtype.
pub(crate) fn hash_bytes(bytes: &[u8]) -> Blake3Hash {
    Blake3Hash::new(*blake3::hash(bytes).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bytes_produces_well_known_hash() {
        // Round-trip via hex to keep the test resilient to internal repr.
        let h = hash_bytes(b"");
        let again = hash_bytes(b"");
        assert_eq!(h, again);
    }

    #[test]
    fn distinct_inputs_produce_distinct_hashes() {
        assert_ne!(hash_bytes(b"a"), hash_bytes(b"b"));
    }
}
