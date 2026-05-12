//! Embedder trait + candle-backed BGE-small implementation.
//!
//! Model: `bge-small-en-v1.5`, 384-dim, ~130 MB. **Not** shipped with the
//! binary — downloaded on first cold start that touches prose, cached at
//! `~/.cache/standardoc/models/bge-small-en-v1.5/` (XDG / platform
//! convention). Override via `STANDARDOC_MODELS_DIR` env var.
//!
//! Pure-Rust candle was picked over `fastembed-rs` to keep the binary
//! free of ONNX Runtime native libs. Trade-off : ~4× slower at cold
//! start than ONNX, imperceptible after the BLAKE3 hash-skip kicks in.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams, TruncationStrategy};

use crate::error::RagError;
use crate::types::EmbedModel;

/// HuggingFace repo slug for BGE-small-en-v1.5. Stored as a const so
/// future migrations (a larger model, a multilingual variant) are a
/// single-line change.
pub const BGE_SMALL_HF_REPO: &str = "BAAI/bge-small-en-v1.5";

/// Producer of 1 embedding vector per input string. Stateless from the
/// caller's POV — the backing weights live inside the impl.
pub trait Embedder: Send + Sync {
    fn model(&self) -> &EmbedModel;
    fn embed(&self, text: &str) -> Result<Vec<f32>, RagError>;
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, RagError>;
}

/// Resolves the directory where the model weights are cached. Order of
/// precedence : `STANDARDOC_MODELS_DIR` env > platform cache dir.
pub fn resolve_models_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("STANDARDOC_MODELS_DIR") {
        return PathBuf::from(custom);
    }
    let base = platform_cache_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("standardoc").join("models")
}

#[cfg(target_os = "windows")]
fn platform_cache_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn platform_cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library").join("Caches"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_cache_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(xdg));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))
}

/// Candle-backed BGE-small embedder. Loads BERT weights from a local
/// `model_dir` containing `config.json`, `tokenizer.json` and
/// `model.safetensors`. Call [`CandleBgeSmall::download`] first if the
/// files are absent.
pub struct CandleBgeSmall {
    model: EmbedModel,
    model_dir: PathBuf,
    bert: Mutex<BertModel>,
    tokenizer: Mutex<Tokenizer>,
    device: Device,
    max_sequence_length: usize,
}

impl CandleBgeSmall {
    /// Default max sequence length used both for tokenizer truncation
    /// and the BERT positional embedding cap. BGE-small supports up to
    /// 512.
    pub const MAX_SEQUENCE_LENGTH: usize = 512;

    /// Loads the model from `model_dir`. Returns `RagError::ModelNotFound`
    /// if any of the three required files is missing. Call
    /// [`CandleBgeSmall::download`] to fetch them.
    pub fn load(model_dir: PathBuf) -> Result<Self, RagError> {
        let tokenizer_path = model_dir.join("tokenizer.json");
        let config_path = model_dir.join("config.json");
        let weights_path = model_dir.join("model.safetensors");

        let any_missing = [&tokenizer_path, &config_path, &weights_path]
            .iter()
            .any(|p| !p.exists());
        if any_missing {
            return Err(RagError::ModelNotFound { path: model_dir });
        }

        let device = Device::Cpu;
        let config_text = std::fs::read_to_string(&config_path)?;
        let config: Config = serde_json::from_str(&config_text).map_err(|e| {
            RagError::Embedder {
                detail: format!("config parse: {e}"),
            }
        })?;

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            RagError::Embedder {
                detail: format!("tokenizer load: {e}"),
            }
        })?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..PaddingParams::default()
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: Self::MAX_SEQUENCE_LENGTH,
                strategy: TruncationStrategy::LongestFirst,
                ..TruncationParams::default()
            }))
            .map_err(|e| RagError::Embedder {
                detail: format!("tokenizer truncation: {e}"),
            })?;

        // SAFETY: the `unsafe` block is gated behind the workspace's
        // `unsafe_code = forbid` ; we cannot use it. Fallback : read the
        // safetensors file into memory (less memory-efficient but safe).
        let weights_bytes = std::fs::read(&weights_path)?;
        let vb = VarBuilder::from_buffered_safetensors(weights_bytes, DTYPE, &device).map_err(
            |e| RagError::Embedder {
                detail: format!("safetensors buffer: {e}"),
            },
        )?;
        let bert = BertModel::load(vb, &config).map_err(|e| RagError::Embedder {
            detail: format!("bert load: {e}"),
        })?;

        Ok(Self {
            model: EmbedModel::bge_small_en_v1_5(),
            model_dir,
            bert: Mutex::new(bert),
            tokenizer: Mutex::new(tokenizer),
            device,
            max_sequence_length: Self::MAX_SEQUENCE_LENGTH,
        })
    }

    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    /// Returns `true` if the three required files exist under `model_dir`
    /// — useful to drive a "download if absent" branch without
    /// hammering `load()`.
    pub fn is_present(model_dir: &Path) -> bool {
        ["config.json", "tokenizer.json", "model.safetensors"]
            .iter()
            .all(|f| model_dir.join(f).exists())
    }

    /// Downloads the model files from HuggingFace Hub if they are not
    /// already present in `model_dir`. Idempotent : existing files are
    /// not re-fetched. Blocks the calling thread for the duration of
    /// the (network-bound) HTTP transfers.
    pub fn download(model_dir: &Path) -> Result<(), RagError> {
        use hf_hub::api::sync::Api;
        std::fs::create_dir_all(model_dir)?;

        let api = Api::new().map_err(|e| RagError::Embedder {
            detail: format!("hf api init: {e}"),
        })?;
        let repo = api.model(BGE_SMALL_HF_REPO.to_string());

        for filename in ["config.json", "tokenizer.json", "model.safetensors"] {
            let dest = model_dir.join(filename);
            if dest.exists() {
                continue;
            }
            let src = repo.get(filename).map_err(|e| RagError::Embedder {
                detail: format!("hf get {filename}: {e}"),
            })?;
            std::fs::copy(&src, &dest)?;
        }
        Ok(())
    }
}

impl Embedder for CandleBgeSmall {
    fn model(&self) -> &EmbedModel {
        &self.model
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, RagError> {
        let mut out = self.embed_batch(&[text])?;
        out.pop().ok_or_else(|| RagError::Embedder {
            detail: "empty batch result".into(),
        })
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, RagError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let owned: Vec<String> = texts.iter().map(|t| (*t).to_string()).collect();

        let encodings = {
            let tokenizer = self.tokenizer.lock().map_err(|_| RagError::Poisoned)?;
            tokenizer
                .encode_batch(owned, true)
                .map_err(|e| RagError::Embedder {
                    detail: format!("encode: {e}"),
                })?
        };

        let batch_size = encodings.len();
        let max_len = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(0)
            .min(self.max_sequence_length);
        if max_len == 0 {
            return Ok(vec![Vec::new(); batch_size]);
        }

        let mut ids: Vec<u32> = Vec::with_capacity(batch_size * max_len);
        let mut attn: Vec<u32> = Vec::with_capacity(batch_size * max_len);
        for enc in &encodings {
            ids.extend_from_slice(enc.get_ids());
            attn.extend_from_slice(enc.get_attention_mask());
        }

        let ids_t = Tensor::from_vec(ids, (batch_size, max_len), &self.device).map_err(|e| {
            RagError::Embedder {
                detail: format!("ids tensor: {e}"),
            }
        })?;
        let attn_t = Tensor::from_vec(attn, (batch_size, max_len), &self.device).map_err(|e| {
            RagError::Embedder {
                detail: format!("attn tensor: {e}"),
            }
        })?;
        let type_ids_t = Tensor::zeros((batch_size, max_len), DType::U32, &self.device).map_err(
            |e| RagError::Embedder {
                detail: format!("type_ids tensor: {e}"),
            },
        )?;

        let hidden = {
            let bert = self.bert.lock().map_err(|_| RagError::Poisoned)?;
            bert.forward(&ids_t, &type_ids_t, Some(&attn_t))
                .map_err(|e| RagError::Embedder {
                    detail: format!("forward: {e}"),
                })?
        };

        let pooled = mean_pool(&hidden, &attn_t)?;
        let normalized = l2_normalize(&pooled)?;

        let mut out = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let row = normalized.get(i).map_err(|e| RagError::Embedder {
                detail: format!("get row {i}: {e}"),
            })?;
            let v: Vec<f32> = row.to_vec1::<f32>().map_err(|e| RagError::Embedder {
                detail: format!("to_vec1 row {i}: {e}"),
            })?;
            out.push(v);
        }
        Ok(out)
    }
}

fn mean_pool(hidden: &Tensor, attention: &Tensor) -> Result<Tensor, RagError> {
    let attention_f = attention.to_dtype(DTYPE).map_err(|e| RagError::Embedder {
        detail: format!("attn to_dtype: {e}"),
    })?;
    let attention_expanded = attention_f.unsqueeze(2).map_err(|e| RagError::Embedder {
        detail: format!("attn unsqueeze: {e}"),
    })?;
    let weighted = hidden
        .broadcast_mul(&attention_expanded)
        .map_err(|e| RagError::Embedder {
            detail: format!("weighted mul: {e}"),
        })?;
    let summed = weighted.sum(1).map_err(|e| RagError::Embedder {
        detail: format!("summed: {e}"),
    })?;
    let counts = attention_f.sum_keepdim(1).map_err(|e| RagError::Embedder {
        detail: format!("counts: {e}"),
    })?;
    summed.broadcast_div(&counts).map_err(|e| RagError::Embedder {
        detail: format!("pooled div: {e}"),
    })
}

fn l2_normalize(t: &Tensor) -> Result<Tensor, RagError> {
    let squared = t.sqr().map_err(|e| RagError::Embedder {
        detail: format!("sqr: {e}"),
    })?;
    let summed = squared.sum_keepdim(1).map_err(|e| RagError::Embedder {
        detail: format!("l2 sum: {e}"),
    })?;
    let norm = summed.sqrt().map_err(|e| RagError::Embedder {
        detail: format!("sqrt: {e}"),
    })?;
    t.broadcast_div(&norm).map_err(|e| RagError::Embedder {
        detail: format!("normalize div: {e}"),
    })
}

// -- Mock embedder for testing the pipeline without candle / model files --

/// Deterministic embedder for unit tests and integration tests that
/// must not depend on having the BGE weights downloaded. Each input
/// text hashes to a stable 384-dim vector via BLAKE3 — no semantic
/// meaning, only stability.
pub struct MockEmbedder {
    model: EmbedModel,
}

impl MockEmbedder {
    pub fn new() -> Self {
        Self {
            model: EmbedModel::bge_small_en_v1_5(),
        }
    }
}

impl Default for MockEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder for MockEmbedder {
    fn model(&self) -> &EmbedModel {
        &self.model
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, RagError> {
        Ok(mock_vector(text, self.model.dim))
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, RagError> {
        Ok(texts.iter().map(|t| mock_vector(t, self.model.dim)).collect())
    }
}

fn mock_vector(text: &str, dim: u32) -> Vec<f32> {
    let dim = usize::try_from(dim).unwrap_or(0);
    if dim == 0 {
        return Vec::new();
    }
    let mut buf = Vec::with_capacity(dim);
    let mut acc = blake3::Hasher::new();
    acc.update(text.as_bytes());
    let mut counter: u32 = 0;
    while buf.len() < dim {
        let mut h = acc.clone();
        h.update(&counter.to_le_bytes());
        let bytes = h.finalize();
        for chunk in bytes.as_bytes().chunks_exact(4) {
            if buf.len() == dim {
                break;
            }
            let raw = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            // Map u32 → [-1, 1) deterministically.
            #[allow(clippy::cast_precision_loss)]
            let scaled = ((raw as f32) / (u32::MAX as f32)).mul_add(2.0, -1.0);
            buf.push(scaled);
        }
        counter += 1;
    }
    // L2-normalize so MockEmbedder output is in the same regime as the
    // real one (unit vectors, ready for cosine).
    let norm = buf.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut buf {
            *x /= norm;
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_models_dir_returns_a_path_terminating_in_models() {
        let p = resolve_models_dir();
        if std::env::var("STANDARDOC_MODELS_DIR").is_err() {
            assert!(p.ends_with("standardoc/models") || p.ends_with("standardoc\\models"));
        }
    }

    #[test]
    fn is_present_false_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!CandleBgeSmall::is_present(dir.path()));
    }

    #[test]
    fn load_returns_model_not_found_when_files_absent() {
        let dir = tempfile::tempdir().unwrap();
        let res = CandleBgeSmall::load(dir.path().to_path_buf());
        assert!(matches!(res, Err(RagError::ModelNotFound { .. })));
    }

    #[test]
    fn mock_embedder_advertises_bge_metadata() {
        let m = MockEmbedder::new();
        assert_eq!(m.model().id, "bge-small-en-v1.5");
        assert_eq!(m.model().dim, 384);
    }

    #[test]
    fn mock_embedder_returns_correct_dimension() {
        let m = MockEmbedder::new();
        let v = m.embed("hello world").unwrap();
        assert_eq!(v.len(), 384);
    }

    #[test]
    fn mock_embedder_is_deterministic() {
        let m = MockEmbedder::new();
        let a = m.embed("the same input").unwrap();
        let b = m.embed("the same input").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn mock_embedder_distinguishes_inputs() {
        let m = MockEmbedder::new();
        let a = m.embed("alpha").unwrap();
        let b = m.embed("bravo").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn mock_embedder_output_is_l2_normalised() {
        let m = MockEmbedder::new();
        let v = m.embed("normalise me").unwrap();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "norm = {norm}");
    }

    #[test]
    fn mock_embed_batch_preserves_order_and_distinctness() {
        let m = MockEmbedder::new();
        let texts = ["alpha", "bravo", "charlie"];
        let out = m.embed_batch(&texts).unwrap();
        assert_eq!(out.len(), 3);
        assert_ne!(out[0], out[1]);
        assert_ne!(out[1], out[2]);
        // Same input at same position → same vector (sanity).
        let v_alpha = m.embed("alpha").unwrap();
        assert_eq!(out[0], v_alpha);
    }

    /// Real candle inference. Ignored by default — requires `cargo test -- --ignored`
    /// AND the model files already downloaded. Run `CandleBgeSmall::download`
    /// against `STANDARDOC_MODELS_DIR` once if you want this to pass locally.
    #[test]
    #[ignore = "requires BGE-small model files downloaded"]
    fn candle_bge_small_real_inference_smoke() {
        let dir = resolve_models_dir().join("bge-small-en-v1.5");
        if !CandleBgeSmall::is_present(&dir) {
            eprintln!("skipping: model not present at {}", dir.display());
            return;
        }
        let embedder = CandleBgeSmall::load(dir).unwrap();
        let v = embedder.embed("the quick brown fox jumps over the lazy dog").unwrap();
        assert_eq!(v.len(), 384);
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "norm = {norm}");
    }
}
