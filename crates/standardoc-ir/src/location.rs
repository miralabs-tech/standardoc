use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Site {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolLocation {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_col: u32,
    pub end_col: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_round_trip() {
        let s = Site {
            file: "src/foo.rs".into(),
            line: 12,
            col: 4,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Site = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn symbol_location_round_trip() {
        let loc = SymbolLocation {
            file: "src/lib.rs".into(),
            start_line: 1,
            end_line: 10,
            start_col: 0,
            end_col: 1,
        };
        let json = serde_json::to_string(&loc).unwrap();
        let back: SymbolLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(loc, back);
    }
}
