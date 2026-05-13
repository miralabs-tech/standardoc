//! Chunk → symbol linker. Three signals, by descending base confidence :
//!
//! 1. **Frontmatter** (`1.0`) — author lists fqdns explicitly in the
//!    file's YAML frontmatter (`--- symbols: [a::b, c::d] ---`).
//! 2. **Auto FQDN exact** (`0.7`) — chunk text contains the full fqdn
//!    `crate::mod::Sym` literally.
//! 3. **Auto name substring** (`0.4`) — chunk text contains the symbol's
//!    short name (length ≥ 4 to drop noisy hits like `new` / `id`).
//!
//! When multiple signals fire for the same `(chunk, fqdn)` pair, the
//! highest-confidence one wins (frontmatter > exact > substring). The
//! `def_site_boost` is then applied on top by `score::apply_def_site_boost`
//! using the symbol's def-site path passed in by the orchestrator.

use std::collections::HashMap;

use crate::error::RagError;
use crate::score::{applies_def_site_boost, apply_def_site_boost, compute_link_confidence};
use crate::types::{ChunkId, ChunkSymbolLink, LinkSource};

/// Minimum length of a short name that gets scanned for substring hits.
/// Shorter names (`new`, `id`, `as`) generate too much noise.
pub const SHORT_NAME_MIN_LEN: usize = 4;

/// Common generic method names filtered from the auto-name-substring scan.
/// A FQDN whose terminal segment (case-insensitive) matches one of these
/// is skipped before the chunk text scan — otherwise verbs like `open` /
/// `load` / `path` produce spurious links from any prose that mentions
/// them in passing (e.g. README "Commands: open the dashboard" → linked
/// to every `Type::open` symbol in the workspace). Items below
/// `SHORT_NAME_MIN_LEN` are already excluded upstream; this list targets
/// the 4+-char common cases. Lowercase, sorted alphabetically.
pub const SHORT_NAME_STOPLIST: &[&str] = &[
    "data", "default", "done", "file", "find", "from", "info", "init", "into", "iter", "kind",
    "load", "make", "name", "next", "node", "open", "path", "read", "self", "take", "text", "with",
];

/// Input fed to the linker per-file. The orchestrator (typically
/// `standardoc-core::pipeline::apply_documents` or its RAG sibling)
/// produces it after running the chunker.
#[derive(Debug, Clone)]
pub struct LinkInput<'a> {
    /// Workspace-relative path of the source `.md` file.
    pub source_path: &'a str,
    /// Raw frontmatter block, if present (everything between the opening
    /// and closing `---` lines, excluding the fences themselves).
    pub frontmatter_raw: Option<&'a str>,
    /// Persisted chunks for this source, with their assigned ids.
    pub chunks: &'a [(ChunkId, &'a str)],
}

/// View on the symbol graph that the linker needs to compute confidence +
/// def-site boost. Kept as a trait so `standardoc-rag` stays decoupled
/// from `standardoc-core` (no reverse dep) ; the orchestrator implements
/// it on top of `query::find_symbol` + `query::list_symbols`.
pub trait SymbolLookup {
    /// Returns every workspace fqdn (used to populate the regex set for
    /// auto-FQDN-exact). External symbols are NOT scanned for in prose
    /// by default — too noisy.
    fn workspace_fqdns(&self) -> Result<Vec<String>, RagError>;

    /// Resolves `fqdn` → workspace-relative def-site path. Used for the
    /// `def_site_boost` multiplier.
    fn def_site_path(&self, fqdn: &str) -> Result<Option<String>, RagError>;
}

/// Linker producing one `Vec<ChunkSymbolLink>` per input. Pure data
/// transformation — persistence is the store's job.
///
/// Object-safe : takes `&dyn SymbolLookup` so callers can store a
/// `Box<dyn Linker>` if needed.
pub trait Linker: Send + Sync {
    fn link(
        &self,
        input: &LinkInput<'_>,
        lookup: &dyn SymbolLookup,
    ) -> Result<Vec<ChunkSymbolLink>, RagError>;
}

pub struct DefaultLinker;

impl DefaultLinker {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for DefaultLinker {
    fn default() -> Self {
        Self::new()
    }
}

impl Linker for DefaultLinker {
    fn link(
        &self,
        input: &LinkInput<'_>,
        lookup: &dyn SymbolLookup,
    ) -> Result<Vec<ChunkSymbolLink>, RagError> {
        let frontmatter_symbols = input
            .frontmatter_raw
            .map(extract_frontmatter_symbols)
            .transpose()?
            .unwrap_or_default();

        let workspace_fqdns = lookup.workspace_fqdns()?;
        let short_names = derive_short_names(&workspace_fqdns);

        let mut links: HashMap<(ChunkId, String), LinkSource> = HashMap::new();
        for (chunk_id, chunk_text) in input.chunks {
            for fqdn in &frontmatter_symbols {
                upsert_dominant(&mut links, *chunk_id, fqdn.clone(), LinkSource::Frontmatter);
            }
            for fqdn in &workspace_fqdns {
                if chunk_text.contains(fqdn.as_str()) {
                    upsert_dominant(
                        &mut links,
                        *chunk_id,
                        fqdn.clone(),
                        LinkSource::AutoFqdnExact,
                    );
                }
            }
            for (fqdn, short) in &short_names {
                if chunk_text.contains(short.as_str()) {
                    upsert_dominant(
                        &mut links,
                        *chunk_id,
                        fqdn.clone(),
                        LinkSource::AutoNameSubstring,
                    );
                }
            }
        }

        let mut output = Vec::with_capacity(links.len());
        for ((chunk_id, fqdn), source) in links {
            let def_site = lookup.def_site_path(&fqdn)?;
            let applies = applies_def_site_boost(input.source_path, def_site.as_deref());
            let link_conf = compute_link_confidence(source);
            let final_conf = apply_def_site_boost(link_conf, applies);
            output.push(ChunkSymbolLink {
                chunk_id,
                fqdn,
                confidence: final_conf,
                source,
                def_site_path: def_site,
            });
        }

        output.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| (a.chunk_id.raw(), &a.fqdn).cmp(&(b.chunk_id.raw(), &b.fqdn)))
        });

        Ok(output)
    }
}

fn upsert_dominant(
    links: &mut HashMap<(ChunkId, String), LinkSource>,
    chunk_id: ChunkId,
    fqdn: String,
    candidate: LinkSource,
) {
    links
        .entry((chunk_id, fqdn))
        .and_modify(|existing| {
            *existing = dominant_source(*existing, candidate);
        })
        .or_insert(candidate);
}

fn derive_short_names(fqdns: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for fqdn in fqdns {
        let short = fqdn.rsplit("::").next().unwrap_or(fqdn);
        if short.len() < SHORT_NAME_MIN_LEN {
            continue;
        }
        if SHORT_NAME_STOPLIST
            .iter()
            .any(|w| w.eq_ignore_ascii_case(short))
        {
            continue;
        }
        out.push((fqdn.clone(), short.to_string()));
    }
    out
}

/// Parses the `symbols: [a::b, c::d]` field of a YAML frontmatter block.
/// Tolerant : accepts inline-array (`symbols: [a, b]`), bare-list
/// (one item per `-` line). Missing field returns empty.
///
/// Custom 40-line reader rather than a full YAML dep — the surface is one
/// field with two shapes, anything more elaborate is out of scope here.
pub fn extract_frontmatter_symbols(frontmatter_raw: &str) -> Result<Vec<String>, RagError> {
    let lines: Vec<&str> = frontmatter_raw.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("symbols:") else {
            continue;
        };
        let after = rest.trim();
        if after.starts_with('[') {
            return Ok(parse_inline_array(after));
        }
        if after.is_empty() {
            return Ok(parse_bare_list(&lines, i + 1));
        }
        // `symbols: foo` (single value, no list) is tolerated as one item.
        return Ok(vec![strip_quotes(after).to_string()]);
    }
    Ok(Vec::new())
}

fn parse_inline_array(s: &str) -> Vec<String> {
    let body = s.trim_start_matches('[').trim_end_matches(']');
    body.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| strip_quotes(item).to_string())
        .collect()
}

fn parse_bare_list(lines: &[&str], start: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in lines.iter().skip(start) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            out.push(strip_quotes(item.trim()).to_string());
        } else {
            break;
        }
    }
    out
}

fn strip_quotes(s: &str) -> &str {
    let trimmed = s.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        let inner = &trimmed[1..trimmed.len().saturating_sub(1)];
        return inner;
    }
    trimmed
}

/// Returns the `LinkSource` with the higher base confidence between `a`
/// and `b`. Used to dedupe when multiple signals fire for the same
/// `(chunk, fqdn)` pair.
pub const fn dominant_source(a: LinkSource, b: LinkSource) -> LinkSource {
    if a.base_confidence() >= b.base_confidence() {
        a
    } else {
        b
    }
}

/// Extracts the body of a YAML frontmatter block (text between the
/// opening and closing `---` fences). Returns `None` if `source` does
/// not begin with `---\n`.
///
/// Lives here rather than in the chunker because the linker is the
/// consumer ; the chunker already includes frontmatter as text in its
/// first chunk (cheap, the linker re-reads it).
pub fn extract_frontmatter_block(source: &str) -> Option<&str> {
    let stripped = source
        .strip_prefix("---\n")
        .or_else(|| source.strip_prefix("---\r\n"))?;
    let end_idx = stripped.find("\n---")?;
    Some(&stripped[..end_idx])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeLookup {
        fqdns: Vec<String>,
        def_sites: HashMap<String, String>,
    }

    impl FakeLookup {
        fn new(fqdns: &[&str]) -> Self {
            Self {
                fqdns: fqdns.iter().map(|s| (*s).to_string()).collect(),
                def_sites: HashMap::new(),
            }
        }
        fn with_def_site(mut self, fqdn: &str, path: &str) -> Self {
            self.def_sites.insert(fqdn.to_string(), path.to_string());
            self
        }
    }

    impl SymbolLookup for FakeLookup {
        fn workspace_fqdns(&self) -> Result<Vec<String>, RagError> {
            Ok(self.fqdns.clone())
        }
        fn def_site_path(&self, fqdn: &str) -> Result<Option<String>, RagError> {
            Ok(self.def_sites.get(fqdn).cloned())
        }
    }

    #[test]
    fn dominant_source_prefers_higher_confidence() {
        assert_eq!(
            dominant_source(LinkSource::Frontmatter, LinkSource::AutoFqdnExact),
            LinkSource::Frontmatter,
        );
        assert_eq!(
            dominant_source(LinkSource::AutoNameSubstring, LinkSource::AutoFqdnExact),
            LinkSource::AutoFqdnExact,
        );
    }

    #[test]
    fn base_confidences_are_strictly_ordered() {
        assert!(
            LinkSource::Frontmatter.base_confidence() > LinkSource::AutoFqdnExact.base_confidence()
        );
        assert!(
            LinkSource::AutoFqdnExact.base_confidence()
                > LinkSource::AutoNameSubstring.base_confidence()
        );
    }

    #[test]
    fn frontmatter_inline_array() {
        let fm = "standardoc: rag\nsymbols: [auth::login, auth::logout]\n";
        let s = extract_frontmatter_symbols(fm).unwrap();
        assert_eq!(
            s,
            vec!["auth::login".to_string(), "auth::logout".to_string()]
        );
    }

    #[test]
    fn frontmatter_inline_array_with_quotes() {
        let fm = "symbols: [\"auth::login\", 'auth::logout']\n";
        let s = extract_frontmatter_symbols(fm).unwrap();
        assert_eq!(
            s,
            vec!["auth::login".to_string(), "auth::logout".to_string()]
        );
    }

    #[test]
    fn frontmatter_bare_list() {
        let fm = "symbols:\n  - auth::login\n  - auth::logout\n";
        let s = extract_frontmatter_symbols(fm).unwrap();
        assert_eq!(
            s,
            vec!["auth::login".to_string(), "auth::logout".to_string()]
        );
    }

    #[test]
    fn frontmatter_bare_list_stops_at_non_dash_line() {
        let fm = "symbols:\n  - one\n  - two\nother: foo\n  - not_a_symbol\n";
        let s = extract_frontmatter_symbols(fm).unwrap();
        assert_eq!(s, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn frontmatter_missing_symbols_field() {
        let fm = "standardoc: rag\ntitle: x\n";
        let s = extract_frontmatter_symbols(fm).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn frontmatter_single_scalar_value() {
        let fm = "symbols: auth::login\n";
        let s = extract_frontmatter_symbols(fm).unwrap();
        assert_eq!(s, vec!["auth::login".to_string()]);
    }

    #[test]
    fn extract_frontmatter_block_strips_fences() {
        let src = "---\nfoo: bar\nbaz: qux\n---\nbody here\n";
        let fm = extract_frontmatter_block(src).unwrap();
        assert!(fm.contains("foo: bar"));
        assert!(fm.contains("baz: qux"));
        assert!(!fm.contains("---"));
    }

    #[test]
    fn extract_frontmatter_block_returns_none_without_opening_fence() {
        assert!(extract_frontmatter_block("body here\n").is_none());
    }

    #[test]
    fn link_auto_fqdn_exact_match() {
        let lookup = FakeLookup::new(&["auth::login", "billing::charge"]);
        let chunk_text = "This module documents auth::login behaviour.";
        let input = LinkInput {
            source_path: "docs/auth.md",
            frontmatter_raw: None,
            chunks: &[(ChunkId(1), chunk_text)],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].fqdn, "auth::login");
        assert_eq!(out[0].source, LinkSource::AutoFqdnExact);
        assert!((out[0].confidence - 0.7).abs() < 1e-6);
    }

    #[test]
    fn link_short_name_substring() {
        let lookup = FakeLookup::new(&["billing::charge_invoice"]);
        let chunk_text = "We call charge_invoice from the controller.";
        let input = LinkInput {
            source_path: "docs/billing.md",
            frontmatter_raw: None,
            chunks: &[(ChunkId(2), chunk_text)],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, LinkSource::AutoNameSubstring);
        assert!((out[0].confidence - 0.4).abs() < 1e-6);
    }

    #[test]
    fn link_drops_short_name_under_minimum_length() {
        let lookup = FakeLookup::new(&["foo::new", "foo::id"]);
        let chunk_text = "new id new id everywhere";
        let input = LinkInput {
            source_path: "docs/x.md",
            frontmatter_raw: None,
            chunks: &[(ChunkId(3), chunk_text)],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert!(out.is_empty(), "names shorter than 4 chars must be skipped");
    }

    #[test]
    fn link_dedup_keeps_highest_confidence_source() {
        let lookup = FakeLookup::new(&["auth::login"]);
        // Same chunk matches BOTH the full fqdn AND the short name.
        let chunk_text = "auth::login is documented here ; the login function ...";
        let input = LinkInput {
            source_path: "docs/auth.md",
            frontmatter_raw: Some("symbols: [auth::login]\n"),
            chunks: &[(ChunkId(4), chunk_text)],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].source,
            LinkSource::Frontmatter,
            "frontmatter must dominate auto signals",
        );
        assert!((out[0].confidence - 1.0).abs() < 1e-6);
    }

    #[test]
    fn link_def_site_boost_applied_when_paths_co_locate() {
        let lookup =
            FakeLookup::new(&["auth::login"]).with_def_site("auth::login", "src/auth/login.rs");
        let chunk_text = "auth::login lives here";
        let input = LinkInput {
            source_path: "docs/auth/login.md",
            frontmatter_raw: None,
            chunks: &[(ChunkId(5), chunk_text)],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert_eq!(out.len(), 1);
        // 0.7 × 1.5 = 1.05 capped to 1.0
        assert!((out[0].confidence - 1.0).abs() < 1e-6);
        assert_eq!(out[0].def_site_path.as_deref(), Some("src/auth/login.rs"));
    }

    #[test]
    fn link_def_site_boost_not_applied_for_unrelated_path() {
        let lookup =
            FakeLookup::new(&["auth::login"]).with_def_site("auth::login", "src/auth/login.rs");
        let input = LinkInput {
            source_path: "docs/random/notes.md",
            frontmatter_raw: None,
            chunks: &[(ChunkId(6), "auth::login mention")],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert_eq!(out.len(), 1);
        assert!((out[0].confidence - 0.7).abs() < 1e-6);
    }

    #[test]
    fn link_sorts_by_confidence_descending() {
        let lookup = FakeLookup::new(&["a::high", "b::medium", "c::low_low_low"]);
        let chunk_text = "a::high b::medium low_low_low";
        let input = LinkInput {
            source_path: "docs/x.md",
            frontmatter_raw: Some("symbols: [a::high]\n"),
            chunks: &[(ChunkId(7), chunk_text)],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert!(!out.is_empty());
        for w in out.windows(2) {
            assert!(w[0].confidence >= w[1].confidence);
        }
        assert_eq!(out[0].fqdn, "a::high");
    }

    #[test]
    fn link_multiple_chunks_distinct_links() {
        let lookup = FakeLookup::new(&["auth::login", "billing::charge"]);
        let input = LinkInput {
            source_path: "docs/x.md",
            frontmatter_raw: None,
            chunks: &[
                (ChunkId(10), "auth::login is one"),
                (ChunkId(11), "billing::charge is another"),
            ],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert_eq!(out.len(), 2);
        let pairs: Vec<_> = out.iter().map(|l| (l.chunk_id, l.fqdn.clone())).collect();
        assert!(pairs.contains(&(ChunkId(10), "auth::login".to_string())));
        assert!(pairs.contains(&(ChunkId(11), "billing::charge".to_string())));
    }

    #[test]
    fn derive_short_names_excludes_stoplist_entries() {
        let fqdns = vec![
            "sessions::SessionsHandle::open".to_string(),
            "core::IndexHandle::load".to_string(),
            "core::Foo::path".to_string(),
            "billing::charge_invoice".to_string(),
            "auth::do_login".to_string(),
        ];
        let pairs = derive_short_names(&fqdns);
        let shorts: Vec<&str> = pairs.iter().map(|(_, s)| s.as_str()).collect();
        assert!(!shorts.contains(&"open"), "stoplisted `open` must be excluded");
        assert!(!shorts.contains(&"load"), "stoplisted `load` must be excluded");
        assert!(!shorts.contains(&"path"), "stoplisted `path` must be excluded");
        assert!(shorts.contains(&"charge_invoice"));
        assert!(shorts.contains(&"do_login"));
    }

    #[test]
    fn derive_short_names_stoplist_is_case_insensitive() {
        let fqdns = vec![
            "weird::Open".to_string(),
            "weird::OPEN".to_string(),
            "weird::oPeN".to_string(),
        ];
        let pairs = derive_short_names(&fqdns);
        assert!(
            pairs.is_empty(),
            "stoplist match must ignore ASCII case, got: {pairs:?}",
        );
    }

    #[test]
    fn link_drops_stoplisted_short_names() {
        let lookup = FakeLookup::new(&["sessions::SessionsHandle::open"]);
        // README-style prose that incidentally mentions `open` — must NOT
        // produce an auto-name-substring link to `SessionsHandle::open`.
        let chunk_text = "Commands: open the dashboard from the menu.";
        let input = LinkInput {
            source_path: "README.md",
            frontmatter_raw: None,
            chunks: &[(ChunkId(42), chunk_text)],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert!(
            out.is_empty(),
            "stoplisted short name `open` must not produce a substring link, got: {out:?}",
        );
    }

    #[test]
    fn link_keeps_frontmatter_signal_even_when_short_name_is_stoplisted() {
        let lookup = FakeLookup::new(&["sessions::SessionsHandle::open"]);
        // Frontmatter is an explicit author-side directive — must override
        // the stoplist filter (which only affects the AutoNameSubstring path).
        let input = LinkInput {
            source_path: "docs/sessions.md",
            frontmatter_raw: Some("symbols: [sessions::SessionsHandle::open]\n"),
            chunks: &[(ChunkId(43), "the session opens here")],
        };
        let out = DefaultLinker.link(&input, &lookup).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, LinkSource::Frontmatter);
        assert_eq!(out[0].fqdn, "sessions::SessionsHandle::open");
    }
}
