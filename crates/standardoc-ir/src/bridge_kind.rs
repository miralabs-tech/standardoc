use serde::{Deserialize, Serialize};

/// Locked vocabulary for [`BridgeKind`] — slugs admitted as built-in
/// without the `custom:` prefix. Anything outside this list (and not
/// prefixed with `custom:`) is rejected at storage-insert time so the
/// 1.0 vocabulary doesn't drift via silent extractor mistakes.
///
/// Grouped into tiers (S/A/B/C/D) by category — kept as a flat array
/// so lookups stay a single linear scan over ~80 entries (cheaper than
/// a HashSet for this cardinality, no allocation, const-eligible).
///
/// **Conventions:**
/// - kebab-case slugs (no underscores, no version suffixes).
/// - ORM-specific slug wins over generic (`prisma` over `sql` when both apply).
/// - Post-1.0 vendor-specific kinds MUST use `custom:vendor:<slug>` rather
///   than landing in this list — see [`BridgeKind::is_custom`].
pub const BUILTIN_BRIDGE_KINDS: &[&str] = &[
    // Tier S — language/runtime bridges (FFI, language bindings, RPC layers
    // that cross language boundaries at the runtime level).
    "tauri",
    "wasm-bindgen",
    "napi",
    "neon",
    "pyo3",
    "cffi",
    "ffi",
    "cxx",
    "uniffi",
    "jni",
    "cgo",
    "mlua",
    "wit",
    "swift-bridge",
    "kotlin-native",
    "electron-ipc",
    "wasi",
    "dotnet-pinvoke",
    "comlink",
    // Tier A — data / ORM bridges (target-shapes describing DB objects + the
    // mechanism that exposes them).
    "db-table",
    "db-view",
    "db-procedure",
    "db-model",
    "db-column",
    "db-index",
    "db-migration",
    "sql",
    "sqlx",
    "diesel",
    "sea-orm",
    "prisma",
    "drizzle",
    "typeorm",
    "mikro-orm",
    "kysely",
    "mongoose",
    "sqlalchemy",
    "django-orm",
    "peewee",
    "gorm",
    "sqlc",
    "ef-core",
    "activerecord",
    // Tier B — API / wire-protocol bridges.
    "rest",
    "graphql",
    "grpc",
    "proto",
    "openapi",
    "trpc",
    "connect-rpc",
    "jsonrpc",
    "soap",
    "webhook",
    "server-actions",
    "route-handler",
    // Tier C — IPC / messaging bridges.
    "redis-pubsub",
    "redis-stream",
    "kafka",
    "nats",
    "rabbitmq",
    "mqtt",
    "aws-sqs",
    "aws-sns",
    "gcp-pubsub",
    "azure-servicebus",
    "cron",
    "temporal",
    "bullmq",
    "sidekiq",
    "celery",
    "unix-socket",
    "pipe",
    // Tier D — embed / config / i18n bridges (static-asset references and
    // out-of-code value bindings).
    "embed-asset",
    "i18n-key",
    "env-var",
    "config-key",
    "feature-flag",
    "secret-ref",
];

/// Prefix marking a vendor-specific bridge kind outside the locked
/// [`BUILTIN_BRIDGE_KINDS`] vocabulary. Required for any non-builtin
/// slug to pass [`BridgeKind::try_validate`] post-1.0.
pub const CUSTOM_BRIDGE_PREFIX: &str = "custom:";

/// Validation error returned by [`BridgeKind::try_validate`]. Storage
/// callers map this to their own error type via `From<BridgeKindError>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeKindError {
    /// Slug is neither in [`BUILTIN_BRIDGE_KINDS`] nor prefixed with
    /// [`CUSTOM_BRIDGE_PREFIX`]. Carries the offending slug verbatim
    /// so error messages can quote it.
    Invalid { slug: String },
}

impl std::fmt::Display for BridgeKindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { slug } => write!(
                f,
                "bridge kind {slug:?} is not in the 1.0 built-in vocabulary and \
                 is not prefixed with `custom:` — either add it to \
                 BUILTIN_BRIDGE_KINDS or rename to `custom:{slug}`"
            ),
        }
    }
}

impl std::error::Error for BridgeKindError {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BridgeKind(pub String);

impl BridgeKind {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `true` when the slug appears verbatim in [`BUILTIN_BRIDGE_KINDS`].
    pub fn is_builtin(&self) -> bool {
        BUILTIN_BRIDGE_KINDS.contains(&self.0.as_str())
    }

    /// `true` when the slug starts with the [`CUSTOM_BRIDGE_PREFIX`]
    /// sentinel. The suffix after the prefix is free-form (`custom:bazel`,
    /// `custom:vendor:internal-rpc`).
    pub fn is_custom(&self) -> bool {
        self.0.starts_with(CUSTOM_BRIDGE_PREFIX)
    }

    /// `true` when the slug is either built-in or custom-prefixed — the
    /// admissible-at-storage predicate. Mirrors the success branch of
    /// [`Self::try_validate`].
    pub fn is_valid(&self) -> bool {
        self.is_builtin() || self.is_custom()
    }

    /// Validate the slug against the 1.0 vocabulary lock. Returns
    /// [`BridgeKindError::Invalid`] when the slug is neither in
    /// [`BUILTIN_BRIDGE_KINDS`] nor prefixed with `custom:`. Storage
    /// layers call this at insert boundaries to refuse extractor drift
    /// before it pollutes the DB.
    pub fn try_validate(&self) -> Result<(), BridgeKindError> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(BridgeKindError::Invalid {
                slug: self.0.clone(),
            })
        }
    }
}

impl From<&str> for BridgeKind {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for BridgeKind {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for BridgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let b = BridgeKind::from("tauri");
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(json, "\"tauri\"");
        let back: BridgeKind = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn display_is_inner() {
        assert_eq!(BridgeKind::from("wasm-bindgen").to_string(), "wasm-bindgen");
    }

    #[test]
    fn is_builtin_recognizes_one_slug_per_tier() {
        // One slug per tier so a tier-table truncation surfaces fast.
        assert!(BridgeKind::from("tauri").is_builtin(), "tier S");
        assert!(BridgeKind::from("prisma").is_builtin(), "tier A (ORM)");
        assert!(BridgeKind::from("db-table").is_builtin(), "tier A (shape)");
        assert!(BridgeKind::from("rest").is_builtin(), "tier B");
        assert!(BridgeKind::from("kafka").is_builtin(), "tier C");
        assert!(BridgeKind::from("embed-asset").is_builtin(), "tier D");
    }

    #[test]
    fn is_builtin_rejects_unknown_and_capitalized_variants() {
        assert!(!BridgeKind::from("Tauri").is_builtin(), "case-sensitive");
        assert!(!BridgeKind::from("tauri_v2").is_builtin(), "version suffix");
        assert!(!BridgeKind::from("totally-made-up").is_builtin());
        assert!(
            !BridgeKind::from("").is_builtin(),
            "empty slug must not match"
        );
    }

    #[test]
    fn is_custom_only_matches_custom_prefix() {
        assert!(BridgeKind::from("custom:bazel").is_custom());
        assert!(BridgeKind::from("custom:vendor:internal-rpc").is_custom());
        assert!(!BridgeKind::from("tauri").is_custom());
        assert!(!BridgeKind::from("customary").is_custom(), "needs the colon");
        assert!(
            !BridgeKind::from("Custom:bazel").is_custom(),
            "prefix is case-sensitive"
        );
    }

    #[test]
    fn try_validate_accepts_builtin_and_custom() {
        BridgeKind::from("tauri").try_validate().unwrap();
        BridgeKind::from("custom:bazel").try_validate().unwrap();
        BridgeKind::from("custom:").try_validate().unwrap(); // empty suffix permitted
    }

    #[test]
    fn try_validate_rejects_unknown_without_custom_prefix() {
        let err = BridgeKind::from("tauri-v2").try_validate().unwrap_err();
        assert_eq!(
            err,
            BridgeKindError::Invalid {
                slug: "tauri-v2".into()
            }
        );
        // The Display impl must surface the slug verbatim AND suggest the
        // `custom:<slug>` rename so the extractor author has a fix path.
        let rendered = err.to_string();
        assert!(rendered.contains("tauri-v2"), "got `{rendered}`");
        assert!(rendered.contains("custom:tauri-v2"), "got `{rendered}`");
    }

    #[test]
    fn builtin_bridge_kinds_is_unique_and_lowercase() {
        // A duplicate slug or stray uppercase entry is a vocabulary bug —
        // catch it at build time rather than relying on review.
        for &slug in BUILTIN_BRIDGE_KINDS {
            assert_eq!(slug, slug.to_lowercase(), "slug must be lowercase: {slug}");
            assert!(!slug.is_empty(), "empty slug in BUILTIN_BRIDGE_KINDS");
            assert!(
                !slug.starts_with(CUSTOM_BRIDGE_PREFIX),
                "built-in slug must not carry custom: prefix — {slug}"
            );
        }
        let mut seen: Vec<&&str> = BUILTIN_BRIDGE_KINDS.iter().collect();
        seen.sort();
        let len_before = seen.len();
        seen.dedup();
        assert_eq!(
            seen.len(),
            len_before,
            "BUILTIN_BRIDGE_KINDS contains duplicate slugs"
        );
    }
}
