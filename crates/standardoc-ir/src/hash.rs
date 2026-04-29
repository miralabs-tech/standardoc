use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Blake3Hash([u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseHashError {
    InvalidLength(usize),
    InvalidChar,
}

impl std::fmt::Display for ParseHashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLength(n) => write!(f, "expected 64 hex chars, got {n}"),
            Self::InvalidChar => f.write_str("invalid hex character"),
        }
    }
}

impl std::error::Error for ParseHashError {}

impl Blake3Hash {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; 32] {
        self.0
    }

    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(nibble_to_char(byte >> 4));
            out.push(nibble_to_char(byte & 0x0f));
        }
        out
    }

    pub fn from_hex(s: &str) -> Result<Self, ParseHashError> {
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return Err(ParseHashError::InvalidLength(bytes.len()));
        }
        let mut out = [0u8; 32];
        for (i, chunk) in bytes.chunks_exact(2).enumerate() {
            let hi = char_to_nibble(chunk[0]).ok_or(ParseHashError::InvalidChar)?;
            let lo = char_to_nibble(chunk[1]).ok_or(ParseHashError::InvalidChar)?;
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

fn nibble_to_char(n: u8) -> char {
    let n = n & 0x0f;
    if n < 10 {
        char::from(b'0' + n)
    } else {
        char::from(b'a' + n - 10)
    }
}

const fn char_to_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl std::fmt::Display for Blake3Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl std::fmt::Debug for Blake3Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Blake3Hash({})", self.to_hex())
    }
}

impl Serialize for Blake3Hash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Blake3Hash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_hex(&s).map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_uniform() {
        let h = Blake3Hash::new([0xab; 32]);
        let hex = h.to_hex();
        assert_eq!(hex, "ab".repeat(32));
        let back = Blake3Hash::from_hex(&hex).unwrap();
        assert_eq!(h, back);
    }

    #[test]
    fn hex_round_trip_varied() {
        let bytes: [u8; 32] = std::array::from_fn(|i| u8::try_from(i).unwrap());
        let h = Blake3Hash::new(bytes);
        let hex = h.to_hex();
        assert_eq!(hex.len(), 64);
        let back = Blake3Hash::from_hex(&hex).unwrap();
        assert_eq!(h, back);
    }

    #[test]
    fn serde_round_trip() {
        let h = Blake3Hash::new([0x42; 32]);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, format!("\"{}\"", "42".repeat(32)));
        let back: Blake3Hash = serde_json::from_str(&json).unwrap();
        assert_eq!(h, back);
    }

    #[test]
    fn parse_invalid_length() {
        assert!(matches!(
            Blake3Hash::from_hex("abc"),
            Err(ParseHashError::InvalidLength(3))
        ));
        assert!(matches!(
            Blake3Hash::from_hex(""),
            Err(ParseHashError::InvalidLength(0))
        ));
    }

    #[test]
    fn parse_invalid_char() {
        let invalid = "z".repeat(64);
        assert!(matches!(
            Blake3Hash::from_hex(&invalid),
            Err(ParseHashError::InvalidChar)
        ));
    }

    #[test]
    fn parse_uppercase_ok() {
        let lower = "ab".repeat(32);
        let upper = "AB".repeat(32);
        assert_eq!(
            Blake3Hash::from_hex(&lower).unwrap(),
            Blake3Hash::from_hex(&upper).unwrap()
        );
    }

    #[test]
    fn debug_is_hex() {
        let h = Blake3Hash::default();
        assert_eq!(format!("{h:?}"), format!("Blake3Hash({})", "0".repeat(64)));
    }
}
