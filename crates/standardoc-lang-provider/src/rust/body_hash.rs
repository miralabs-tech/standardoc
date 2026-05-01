use proc_macro2::TokenStream;
use standardoc_ir::Blake3Hash;

pub(crate) fn hash_tokens(stream: &TokenStream) -> Blake3Hash {
    let s = stream.to_string();
    let digest = blake3::hash(s.as_bytes());
    Blake3Hash::new(*digest.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn ts(s: &str) -> TokenStream {
        TokenStream::from_str(s).expect("test token stream not parsable")
    }

    #[test]
    fn same_tokens_same_hash() {
        let a = ts("fn foo() { let x = 1; }");
        let b = ts("fn foo() { let x = 1; }");
        assert_eq!(hash_tokens(&a), hash_tokens(&b));
    }

    #[test]
    fn different_tokens_different_hash() {
        let a = ts("fn foo() { let x = 1; }");
        let b = ts("fn foo() { let x = 2; }");
        assert_ne!(hash_tokens(&a), hash_tokens(&b));
    }

    #[test]
    fn whitespace_normalized_via_token_stream() {
        let a = ts("fn   foo (  ) {  let  x  = 1; }");
        let b = ts("fn foo() { let x = 1; }");
        assert_eq!(hash_tokens(&a), hash_tokens(&b));
    }

    #[test]
    fn trailing_comments_ignored_via_token_stream() {
        let a = ts("fn foo() { let x = 1; }");
        let b = ts("fn foo() { let x = 1; } // trailing");
        assert_eq!(hash_tokens(&a), hash_tokens(&b));
    }

    #[test]
    fn leading_doc_comments_change_hash() {
        // doc comments survive as `#[doc = "..."]` attrs in token stream.
        // Body-level inner doc comments are part of the tokens — therefore differ.
        let a = ts("fn foo() { /// inner\n let x = 1; }");
        let b = ts("fn foo() { let x = 1; }");
        assert_ne!(hash_tokens(&a), hash_tokens(&b));
    }
}
