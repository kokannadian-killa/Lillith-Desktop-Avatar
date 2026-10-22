//! Minimal GGUF header reader (pure std).
//!
//! Reads the magic, version and metadata key/values only; tensor data is
//! never touched. Large arrays (the tokenizer vocab) are skipped by seeking,
//! so a header read costs a few MB of sequential I/O at most.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub const GGUF_MAGIC: [u8; 4] = *b"GGUF";
/// Strings longer than this are skipped (the chat template is a few KB).
const MAX_STRING: u64 = 256 * 1024;
/// Arrays longer than this are skipped instead of collected.
const MAX_ARRAY_KEEP: u64 = 64;
/// Sanity cap on the number of metadata entries.
const MAX_KV: u64 = 1_000_000;

#[derive(Debug, thiserror::Error)]
pub enum GgufError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a GGUF file (magic {0:02x?})")]
    BadMagic([u8; 4]),
    #[error("unsupported GGUF version {0}")]
    Version(u32),
    #[error("corrupt header: {0}")]
    Corrupt(String),
}

/// A metadata value. Big arrays are kept only as their length.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value {
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
    Array(Vec<Value>),
    /// An array that was too long to keep, or a skipped oversized string.
    Skipped {
        len: u64,
    },
}

impl Value {
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Value::U64(v) => Some(v),
            Value::I64(v) if v >= 0 => Some(v as u64),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Parsed GGUF header.
#[derive(Debug, Clone, Serialize)]
pub struct GgufHeader {
    pub version: u32,
    pub tensor_count: u64,
    pub metadata: BTreeMap<String, Value>,
}

impl GgufHeader {
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).and_then(Value::as_str)
    }
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.metadata.get(key).and_then(Value::as_u64)
    }
    /// `general.architecture` (e.g. `llama`, `qwen2`).
    pub fn architecture(&self) -> Option<&str> {
        self.get_str("general.architecture")
    }
    pub fn name(&self) -> Option<&str> {
        self.get_str("general.name")
    }
    /// `<arch>.context_length` (training context).
    pub fn context_length(&self) -> Option<u64> {
        self.get_u64(&format!("{}.context_length", self.architecture()?))
    }
    /// `general.file_type` (llama_ftype).
    pub fn file_type(&self) -> Option<u64> {
        self.get_u64("general.file_type")
    }
    /// `tokenizer.chat_template` (Jinja), if embedded.
    pub fn chat_template(&self) -> Option<&str> {
        self.get_str("tokenizer.chat_template")
    }
}

/// True if `path` starts with the GGUF magic (cheap sniff, 4 bytes).
pub fn is_gguf(path: &Path) -> bool {
    let mut m = [0u8; 4];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut m))
        .is_ok()
        && m == GGUF_MAGIC
}

/// Reads the header of the GGUF file at `path`.
pub fn read_header(path: &Path) -> Result<GgufHeader, GgufError> {
    let f = File::open(path)?;
    let len = f.metadata()?.len();
    read_header_from(BufReader::with_capacity(256 * 1024, f), len)
}

/// Reads a header from any seekable reader of total length `len`.
pub fn read_header_from<R: Read + Seek>(r: R, len: u64) -> Result<GgufHeader, GgufError> {
    let mut p = Parser { r, len, v1: false };
    let mut magic = [0u8; 4];
    p.r.read_exact(&mut magic)?;
    if magic != GGUF_MAGIC {
        return Err(GgufError::BadMagic(magic));
    }
    let version = p.u32()?;
    if !(1..=3).contains(&version) {
        return Err(GgufError::Version(version));
    }
    p.v1 = version == 1;
    let tensor_count = p.count()?;
    let kv_count = p.count()?;
    if kv_count > MAX_KV {
        return Err(GgufError::Corrupt(format!("{kv_count} metadata entries")));
    }
    let mut metadata = BTreeMap::new();
    for _ in 0..kv_count {
        let key = match p.string()? {
            Value::Str(s) => s,
            _ => return Err(GgufError::Corrupt("oversized key".into())),
        };
        let ty = p.u32()?;
        let v = p.value(ty, true)?;
        metadata.insert(key, v);
    }
    Ok(GgufHeader {
        version,
        tensor_count,
        metadata,
    })
}

struct Parser<R> {
    r: R,
    len: u64,
    v1: bool,
}

impl<R: Read + Seek> Parser<R> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], GgufError> {
        let mut b = [0u8; N];
        self.r.read_exact(&mut b)?;
        Ok(b)
    }
    fn u32(&mut self) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn u64(&mut self) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }
    /// Counts / lengths are u32 in GGUF v1, u64 afterwards.
    fn count(&mut self) -> Result<u64, GgufError> {
        if self.v1 {
            Ok(self.u32()? as u64)
        } else {
            self.u64()
        }
    }
    fn skip(&mut self, n: u64) -> Result<(), GgufError> {
        let pos = self.r.stream_position()?;
        if pos.saturating_add(n) > self.len {
            return Err(GgufError::Corrupt(format!("skip {n} past end at {pos}")));
        }
        if n <= 64 * 1024 {
            // Small skips (vocab strings) read through the buffer: seeking a
            // BufReader drops its buffer and costs a syscall per token.
            let copied = std::io::copy(&mut (&mut self.r).take(n), &mut std::io::sink())?;
            if copied != n {
                return Err(GgufError::Corrupt("unexpected end of file".into()));
            }
        } else {
            self.r.seek(SeekFrom::Current(n as i64))?;
        }
        Ok(())
    }
    fn string(&mut self) -> Result<Value, GgufError> {
        let n = self.count()?;
        if n > MAX_STRING {
            self.skip(n)?;
            return Ok(Value::Skipped { len: n });
        }
        let mut b = vec![0u8; n as usize];
        self.r.read_exact(&mut b)?;
        Ok(Value::Str(String::from_utf8_lossy(&b).into_owned()))
    }
    fn value(&mut self, ty: u32, keep: bool) -> Result<Value, GgufError> {
        Ok(match ty {
            0 => Value::U64(self.bytes::<1>()?[0] as u64),
            1 => Value::I64(self.bytes::<1>()?[0] as i8 as i64),
            2 => Value::U64(u16::from_le_bytes(self.bytes()?) as u64),
            3 => Value::I64(i16::from_le_bytes(self.bytes()?) as i64),
            4 => Value::U64(self.u32()? as u64),
            5 => Value::I64(i32::from_le_bytes(self.bytes()?) as i64),
            6 => Value::F64(f32::from_le_bytes(self.bytes()?) as f64),
            7 => Value::Bool(self.bytes::<1>()?[0] != 0),
            8 => {
                if keep {
                    self.string()?
                } else {
                    let n = self.count()?;
                    self.skip(n)?;
                    Value::Skipped { len: n }
                }
            }
            9 => {
                let ety = self.u32()?;
                let n = self.count()?;
                let keep_items = keep && n <= MAX_ARRAY_KEEP;
                if !keep_items {
                    if let Some(sz) = fixed_size(ety) {
                        self.skip(n.saturating_mul(sz))?;
                    } else {
                        for _ in 0..n {
                            self.value(ety, false)?;
                        }
                    }
                    return Ok(Value::Skipped { len: n });
                }
                let mut items = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    items.push(self.value(ety, true)?);
                }
                Value::Array(items)
            }
            10 => Value::U64(self.u64()?),
            11 => Value::I64(i64::from_le_bytes(self.bytes()?)),
            12 => Value::F64(f64::from_le_bytes(self.bytes()?)),
            t => return Err(GgufError::Corrupt(format!("unknown value type {t}"))),
        })
    }
}

fn fixed_size(ty: u32) -> Option<u64> {
    match ty {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4..=6 => Some(4),
        10..=12 => Some(8),
        _ => None,
    }
}

/// Human name of a `general.file_type` (llama_ftype) value.
pub fn file_type_name(ft: u64) -> Option<&'static str> {
    Some(match ft {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        _ => return None,
    })
}

/// Quant type guessed from a file name (`...-q4_k_m.gguf` -> `Q4_K_M`).
pub fn quant_from_filename(name: &str) -> Option<String> {
    const KNOWN: &[&str] = &[
        "IQ2_XXS", "IQ3_XXS", "IQ2_XS", "IQ3_XS", "IQ4_XS", "IQ4_NL", "IQ1_S", "IQ1_M", "IQ2_S",
        "IQ2_M", "IQ3_S", "IQ3_M", "Q2_K_S", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q4_K_S", "Q4_K_M",
        "Q5_K_S", "Q5_K_M", "Q2_K", "Q3_K", "Q4_K", "Q5_K", "Q6_K", "Q8_0", "Q4_0", "Q4_1", "Q5_0",
        "Q5_1", "TQ1_0", "TQ2_0", "BF16", "F16", "F32",
    ];
    let up = name.to_ascii_uppercase().replace('-', "_");
    KNOWN
        .iter()
        .find(|k| {
            up.match_indices(*k).any(|(i, _)| {
                let before = up[..i].chars().last();
                let after = up[i + k.len()..].chars().next();
                !before.is_some_and(|c| c.is_ascii_alphanumeric())
                    && !after.is_some_and(|c| c.is_ascii_alphanumeric())
            })
        })
        .map(|k| k.to_string())
}

/// Approximate bits per weight of a quant type (for RAM / parameter estimates).
pub fn bits_per_weight(quant: &str) -> f64 {
    match quant.to_ascii_uppercase().as_str() {
        "F32" => 32.0,
        "F16" | "BF16" => 16.0,
        "Q8_0" => 8.5,
        "Q6_K" => 6.56,
        "Q5_K_M" | "Q5_K" => 5.69,
        "Q5_K_S" | "Q5_1" => 5.54,
        "Q5_0" => 5.5,
        "Q4_K_M" | "Q4_K" => 4.85,
        "Q4_K_S" => 4.58,
        "Q4_1" => 5.0,
        "Q4_0" | "IQ4_NL" => 4.5,
        "IQ4_XS" => 4.25,
        "Q3_K_L" => 4.27,
        "Q3_K_M" | "Q3_K" => 3.91,
        "Q3_K_S" | "IQ3_M" | "IQ3_S" => 3.5,
        "IQ3_XS" | "IQ3_XXS" => 3.1,
        "Q2_K" | "Q2_K_S" => 2.96,
        "IQ2_M" | "IQ2_S" | "IQ2_XS" | "IQ2_XXS" => 2.4,
        "IQ1_S" | "IQ1_M" | "TQ1_0" | "TQ2_0" => 1.8,
        _ => 4.85,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    /// Builds a small synthetic GGUF v3 header.
    pub fn build_gguf(kvs: &[(&str, TestVal)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"GGUF");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes()); // tensors
        b.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
        for (k, v) in kvs {
            put_str(&mut b, k);
            match v {
                TestVal::Str(s) => {
                    b.extend_from_slice(&8u32.to_le_bytes());
                    put_str(&mut b, s);
                }
                TestVal::U32(n) => {
                    b.extend_from_slice(&4u32.to_le_bytes());
                    b.extend_from_slice(&n.to_le_bytes());
                }
                TestVal::StrArray(n) => {
                    b.extend_from_slice(&9u32.to_le_bytes());
                    b.extend_from_slice(&8u32.to_le_bytes());
                    b.extend_from_slice(&(*n as u64).to_le_bytes());
                    for i in 0..*n {
                        put_str(&mut b, &format!("tok{i}"));
                    }
                }
                TestVal::F32Array(n) => {
                    b.extend_from_slice(&9u32.to_le_bytes());
                    b.extend_from_slice(&6u32.to_le_bytes());
                    b.extend_from_slice(&(*n as u64).to_le_bytes());
                    for i in 0..*n {
                        b.extend_from_slice(&(i as f32).to_le_bytes());
                    }
                }
            }
        }
        b
    }
    fn put_str(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&(s.len() as u64).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
    }
    pub enum TestVal {
        Str(&'static str),
        U32(u32),
        StrArray(usize),
        F32Array(usize),
    }

    pub fn sample() -> Vec<u8> {
        build_gguf(&[
            ("general.architecture", TestVal::Str("llama")),
            ("general.name", TestVal::Str("Dolphin 3.0 Llama 3.2 3B")),
            ("llama.context_length", TestVal::U32(131072)),
            ("general.file_type", TestVal::U32(15)),
            ("tokenizer.ggml.tokens", TestVal::StrArray(5000)),
            ("tokenizer.ggml.scores", TestVal::F32Array(5000)),
            ("tokenizer.chat_template", TestVal::Str("{% for m in messages %}<|im_start|>{{ m.role }}\n{{ m.content }}<|im_end|>{% endfor %}")),
            ("after.big.arrays", TestVal::U32(7)),
        ])
    }

    #[test]
    fn reads_synthetic_header_and_skips_big_arrays() {
        let b = sample();
        let len = b.len() as u64;
        let h = read_header_from(Cursor::new(b), len).unwrap();
        assert_eq!(h.version, 3);
        assert_eq!(h.architecture(), Some("llama"));
        assert_eq!(h.name(), Some("Dolphin 3.0 Llama 3.2 3B"));
        assert_eq!(h.context_length(), Some(131072));
        assert_eq!(h.file_type().and_then(file_type_name), Some("Q4_K_M"));
        assert_eq!(
            h.metadata["tokenizer.ggml.tokens"],
            Value::Skipped { len: 5000 }
        );
        assert_eq!(h.get_u64("after.big.arrays"), Some(7));
        assert!(h.chat_template().unwrap().contains("<|im_start|>"));
    }

    #[test]
    fn rejects_non_gguf_and_truncated() {
        let e = read_header_from(Cursor::new(b"PK\x03\x04rest".to_vec()), 8).unwrap_err();
        assert!(matches!(e, GgufError::BadMagic(_)));
        let mut b = sample();
        b.truncate(60);
        let len = b.len() as u64;
        assert!(read_header_from(Cursor::new(b), len).is_err());
    }

    #[test]
    fn quant_names_from_filenames() {
        assert_eq!(
            quant_from_filename("dolphin3.0-llama3.2-3b-q4_k_m.gguf").as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(
            quant_from_filename("Qwen2.5-0.5B-Instruct-Q8_0.gguf").as_deref(),
            Some("Q8_0")
        );
        assert_eq!(
            quant_from_filename("phi-3-mini-4k-instruct-IQ4_XS.gguf").as_deref(),
            Some("IQ4_XS")
        );
        assert_eq!(
            quant_from_filename("model-f16.gguf").as_deref(),
            Some("F16")
        );
        assert_eq!(quant_from_filename("model.gguf"), None);
    }
}
