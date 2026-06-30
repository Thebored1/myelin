//! Minimal GGUF metadata reader — just enough to estimate the KV-cache cost per
//! token and the model's trained context length, so the launcher can size the
//! context to fit system RAM. No external dependency; we parse the header and
//! skip everything we don't need (including the large tokenizer arrays).

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

const GGUF_MAGIC: u32 = 0x4655_4747; // "GGUF" little-endian

#[derive(Debug, Default, Clone)]
pub struct GgufInfo {
    pub architecture: Option<String>,
    pub n_layers: Option<u64>,
    pub embedding_length: Option<u64>,
    pub head_count: Option<u64>,
    pub head_count_kv: Option<u64>,
    pub key_length: Option<u64>,
    pub context_length: Option<u64>,
    /// Whether the model embeds a chat template (needed for `--jinja`). Models
    /// without one must not be launched with `--jinja` or the server fails.
    pub has_chat_template: bool,
}

impl GgufInfo {
    /// Head dimension: explicit `key_length` if present, else embedding/head_count.
    fn head_dim(&self) -> Option<u64> {
        if let Some(k) = self.key_length {
            return Some(k);
        }
        match (self.embedding_length, self.head_count) {
            (Some(e), Some(h)) if h > 0 => Some(e / h),
            _ => None,
        }
    }

    /// True for recurrent (Mamba/RWKV) or hybrid (Mamba+attention) architectures.
    /// These keep a small FIXED recurrent state instead of a KV cache that grows
    /// per token across every layer, so the transformer KV-size formula does not
    /// apply and context is bounded by the model's trained length, not RAM.
    /// Matched by GGUF `general.architecture`; substrings cover family variants
    /// (mamba/mamba2, rwkv6/rwkv7, granitehybrid, falcon_h1, nemotron_h, …).
    pub fn is_recurrent_or_hybrid(&self) -> bool {
        let arch = match &self.architecture {
            Some(a) => a.to_lowercase(),
            None => return false,
        };
        const FAMILIES: &[&str] = &[
            "mamba", "rwkv", "jamba", "granitehybrid", "falcon_h1", "falcon-h1",
            "nemotron_h", "nemotronh", "bamba", "plamo2", "lfm2",
        ];
        arch.contains("hybrid") || FAMILIES.iter().any(|f| arch.contains(f))
    }

    /// Conservative estimate of KV-cache bytes per token (f16 K+V). Falls back to
    /// the full embedding width (i.e. assumes no GQA) when head geometry is
    /// missing, which over-estimates — and over-estimating only makes the
    /// context clamp *safer*. Returns `None` for recurrent/hybrid archs (no
    /// per-token KV growth), so the launcher sizes context by trained length.
    pub fn kv_bytes_per_token(&self) -> Option<u64> {
        if self.is_recurrent_or_hybrid() {
            return None;
        }
        let n_layers = self.n_layers?;
        let kv_dim = match (self.head_count_kv, self.head_dim()) {
            (Some(kv), Some(hd)) => kv.saturating_mul(hd),
            _ => self.embedding_length?,
        };
        // 2 (K and V) * layers * kv_dim * 2 bytes (f16)
        Some(2u64.saturating_mul(n_layers).saturating_mul(kv_dim).saturating_mul(2))
    }
}

/// Read the metadata we care about from a GGUF file. Best-effort: returns
/// whatever fields were found.
pub fn read_gguf_info(path: &Path) -> io::Result<GgufInfo> {
    let mut r = BufReader::new(File::open(path)?);

    if read_u32(&mut r)? != GGUF_MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a GGUF file"));
    }
    let _version = read_u32(&mut r)?;
    let _tensor_count = read_u64(&mut r)?;
    let kv_count = read_u64(&mut r)?;

    let mut arch: Option<String> = None;
    let mut has_chat_template = false;
    let mut ints: HashMap<String, u64> = HashMap::new();

    for _ in 0..kv_count {
        let key = read_gguf_string(&mut r)?;
        let vtype = read_u32(&mut r)?;
        match vtype {
            8 => {
                let s = read_gguf_string(&mut r)?;
                if key == "general.architecture" {
                    arch = Some(s);
                } else if key == "tokenizer.chat_template" && !s.trim().is_empty() {
                    has_chat_template = true;
                }
            }
            0 => {
                ints.insert(key, read_n::<1>(&mut r)?[0] as u64);
            }
            1 => {
                ints.insert(key, read_n::<1>(&mut r)?[0] as i8 as i64 as u64);
            }
            2 => {
                ints.insert(key, read_u16(&mut r)? as u64);
            }
            3 => {
                ints.insert(key, i16::from_le_bytes(read_n::<2>(&mut r)?) as i64 as u64);
            }
            4 => {
                ints.insert(key, read_u32(&mut r)? as u64);
            }
            5 => {
                ints.insert(key, i32::from_le_bytes(read_n::<4>(&mut r)?) as i64 as u64);
            }
            10 => {
                ints.insert(key, read_u64(&mut r)?);
            }
            11 => {
                ints.insert(key, i64::from_le_bytes(read_n::<8>(&mut r)?) as u64);
            }
            6 => skip(&mut r, 4)?,  // float32
            12 => skip(&mut r, 8)?, // float64
            7 => skip(&mut r, 1)?,  // bool
            9 => skip_array(&mut r)?,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown GGUF value type {other}"),
                ))
            }
        }
    }

    let mut info = GgufInfo::default();
    info.has_chat_template = has_chat_template;
    if let Some(a) = arch {
        let get = |suffix: &str| ints.get(&format!("{a}.{suffix}")).copied();
        info.n_layers = get("block_count");
        info.embedding_length = get("embedding_length");
        info.head_count = get("attention.head_count");
        info.head_count_kv = get("attention.head_count_kv");
        info.key_length = get("attention.key_length");
        info.context_length = get("context_length");
        info.architecture = Some(a);
    }
    Ok(info)
}

fn read_n<const N: usize>(r: &mut impl Read) -> io::Result<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b)?;
    Ok(b)
}

fn read_u16(r: &mut impl Read) -> io::Result<u16> {
    Ok(u16::from_le_bytes(read_n::<2>(r)?))
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    Ok(u32::from_le_bytes(read_n::<4>(r)?))
}

fn read_u64(r: &mut impl Read) -> io::Result<u64> {
    Ok(u64::from_le_bytes(read_n::<8>(r)?))
}

fn read_gguf_string(r: &mut impl Read) -> io::Result<String> {
    let len = read_u64(r)?;
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Discard `n` bytes from the stream (buffered, no allocation).
fn skip(r: &mut impl Read, n: u64) -> io::Result<()> {
    let copied = io::copy(&mut r.by_ref().take(n), &mut io::sink())?;
    if copied != n {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(arch: &str, n_layers: u64, kv: u64, hd: u64) -> GgufInfo {
        GgufInfo {
            architecture: Some(arch.into()),
            n_layers: Some(n_layers),
            head_count_kv: Some(kv),
            key_length: Some(hd),
            ..Default::default()
        }
    }

    #[test]
    fn recurrent_hybrid_detection() {
        assert!(info("granitehybrid", 40, 4, 64).is_recurrent_or_hybrid());
        assert!(info("lfm2", 16, 8, 64).is_recurrent_or_hybrid());
        assert!(info("mamba2", 24, 0, 0).is_recurrent_or_hybrid());
        assert!(info("rwkv7", 24, 0, 0).is_recurrent_or_hybrid());
        assert!(info("falcon_h1", 36, 8, 64).is_recurrent_or_hybrid());
        assert!(!info("llama", 32, 8, 128).is_recurrent_or_hybrid());
        assert!(!info("qwen2", 28, 4, 128).is_recurrent_or_hybrid());
        assert!(!GgufInfo::default().is_recurrent_or_hybrid());
    }

    #[test]
    fn kv_none_for_recurrent_some_for_transformer() {
        // Hybrid/recurrent → no transformer KV (context sized by trained length).
        assert!(info("granitehybrid", 40, 4, 64).kv_bytes_per_token().is_none());
        // Transformer → 2 (K+V) * layers * (kv_heads*head_dim) * 2 bytes (f16).
        assert_eq!(
            info("llama", 32, 8, 128).kv_bytes_per_token(),
            Some(2 * 32 * (8 * 128) * 2)
        );
    }
}

/// Skip a GGUF array value (element type + count + elements).
fn skip_array(r: &mut impl Read) -> io::Result<()> {
    let elem_type = read_u32(r)?;
    let count = read_u64(r)?;
    match elem_type {
        0 | 1 | 7 => skip(r, count)?,
        2 | 3 => skip(r, count.saturating_mul(2))?,
        4 | 5 | 6 => skip(r, count.saturating_mul(4))?,
        10 | 11 | 12 => skip(r, count.saturating_mul(8))?,
        8 => {
            for _ in 0..count {
                let l = read_u64(r)?;
                skip(r, l)?;
            }
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported GGUF array element type {other}"),
            ))
        }
    }
    Ok(())
}
