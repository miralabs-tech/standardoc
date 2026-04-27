//! Syntect-backed code highlighting.
//!
//! Exposed as a library function so `standardoc-server/src/web.rs` can call
//! it when rendering markdown pages server-side. Syntax set and theme set are
//! lazy-initialised once and reused across requests.

use std::sync::OnceLock;
use syntect::highlighting::ThemeSet;
use syntect::html::{css_for_theme_with_class_style, ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// CSS class prefix — keeps our classes namespaced and avoids collisions.
pub const HL_PREFIX: &str = "hl-";
const CLASS_STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: HL_PREFIX };

static SS: OnceLock<SyntaxSet> = OnceLock::new();
static TS: OnceLock<ThemeSet> = OnceLock::new();
static CSS: OnceLock<String> = OnceLock::new();

fn ss() -> &'static SyntaxSet {
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn ts() -> &'static ThemeSet {
    TS.get_or_init(ThemeSet::load_defaults)
}

/// Highlight `code` for `lang` (e.g. `"rust"`, `"typescript"`, `"python"`).
/// Returns a `<pre class="code-block"><code>…</code></pre>` HTML string with
/// syntect class spans. Falls back to plain-text if the language is unknown.
pub fn highlight_code(code: &str, lang: Option<&str>) -> String {
    let ss = ss();
    let syntax = lang
        .and_then(|l| ss.find_syntax_by_token(l))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut gen = ClassedHTMLGenerator::new_with_class_style(syntax, ss, CLASS_STYLE);
    for line in LinesWithEndings::from(code) {
        // Errors here are benign (malformed highlight state) — just skip.
        let _ = gen.parse_html_for_line_which_includes_newline(line);
    }
    let inner = gen.finalize();

    let lang_attr = lang
        .map(|l| format!(" data-lang=\"{}\"", html_escape_attr(l)))
        .unwrap_or_default();
    format!("<pre class=\"code-block\"{lang_attr}><code>{inner}</code></pre>")
}

/// Combined light + dark CSS for the syntect class spans.
/// Generated once at first call and cached.
///
/// Light: InspiredGitHub (clean, white background).
/// Dark:  base16-ocean.dark (standard dark palette).
///
/// The dark CSS is wrapped in `@media (prefers-color-scheme: dark)` so the
/// browser picks the right colours automatically. The frontend links to
/// `GET /api/syntax.css` which returns this string.
pub fn syntax_css() -> &'static str {
    CSS.get_or_init(|| {
        let ts = ts();
        let light_css = ts
            .themes
            .get("InspiredGitHub")
            .and_then(|t| css_for_theme_with_class_style(t, CLASS_STYLE).ok())
            .unwrap_or_default();
        let dark_css = ts
            .themes
            .get("base16-ocean.dark")
            .and_then(|t| css_for_theme_with_class_style(t, CLASS_STYLE).ok())
            .unwrap_or_default();
        format!(
            "{light_css}\n@media (prefers-color-scheme: dark) {{\n{dark_css}\n}}\n\
             .code-block {{ overflow-x: auto; border-radius: 6px; padding: 1rem 1.25rem; \
             font-size: 0.875rem; line-height: 1.6; margin: 1rem 0; }}\n\
             .code-block code {{ background: none; padding: 0; border: none; }}"
        )
    })
}

/// Pre-warm the lazy statics at server boot to avoid latency on first request.
/// Syntect loads ~3 MB of grammar data — doing it at startup keeps the first
/// page load fast.
pub fn prewarm() {
    let _ = ss();
    let _ = ts();
    let _ = syntax_css();
}

fn html_escape_attr(s: &str) -> String {
    s.replace('"', "&quot;")
}
