//! Minimal GGUF header reader (pure std).
//!
//! Reads the magic, version and metadata key/values only; tensor data is
//! never touched. Large arrays (the tokenizer vocab) are skipped by seeking,
//! so a header read costs a few MB of sequential I/O at most.
//!
//! Hostile / corrupt files are handled without panics or big allocations:
//! every read is bounds-checked against the file length *before* any buffer
//! is allocated, the whole header must fit in [`MAX_HEADER_BYTES`], counts
//! are capped ([`MAX_KV`], [`MAX_TENSORS`]), array nesting is capped
//! ([`MAX_ARRAY_DEPTH`]), unknown value types are an error, and only regular
//! files are opened (a directory, FIFO or device named `*.gguf` is refused
//! without blocking). Invalid UTF-8 in keys / strings is replaced (lossy),
//! not an error. Big-endian GGUF (rare, s390x builds) is reported as
//! unsupported.

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
/// Sanity cap on the number of metadata entries (real models: ~20–60).
pub const MAX_KV: u64 = 65_536;
/// Sanity cap on the tensor count (real models: hundreds to a few thousand).
pub const MAX_TENSORS: u64 = 1 << 24;
/// The metadata section must end within this many bytes of the start (a
/// 256k-token vocab plus merges is ~10 MB).
pub const MAX_HEADER_BYTES: u64 = 64 * 1024 * 1024;
/// Arrays of arrays deeper than this are refused (real files use none).
pub const MAX_ARRAY_DEPTH: u32 = 4;

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
    Skipped { len: u64 },
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

/// Opens `path` only if it is a regular file (following symlinks), so a
/// FIFO or device never blocks the scanner and a directory is a clean error.
fn open_regular(path: &Path) -> std::io::Result<(File, u64)> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"));
    }
    let f = File::open(path)?;
    let len = f.metadata()?.len();
    Ok((f, len))
}

/// True if `path` is a regular file starting with the GGUF magic (cheap
/// sniff, 4 bytes).
pub fn is_gguf(path: &Path) -> bool {
    let mut m = [0u8; 4];
    open_regular(path).and_then(|(mut f, _)| f.read_exact(&mut m)).is_ok() && m == GGUF_MAGIC
}

/// Reads the header of the GGUF file at `path` (bounded: at most
/// [`MAX_HEADER_BYTES`] are ever read).
pub fn read_header(path: &Path) -> Result<GgufHeader, GgufError> {
    let (f, len) = open_regular(path)?;
    read_header_from(BufReader::with_capacity(256 * 1024, f), len)
}

/// Reads a header from any seekable reader of total length `len`,
/// positioned at the start of the file.
pub fn read_header_from<R: Read + Seek>(r: R, len: u64) -> Result<GgufHeader, GgufError> {
    let mut p = Parser { r, len, pos: 0, v1: false };
    let magic: [u8; 4] = p.bytes()?;
    if magic != GGUF_MAGIC {
        return Err(GgufError::BadMagic(magic));
    }
    let version = p.u32()?;
    if !(1..=3).contains(&version) {
        if (1..=3).contains(&version.swap_bytes()) {
            return Err(GgufError::Corrupt("big-endian GGUF is not supported".into()));
        }
        return Err(GgufError::Version(version));
    }
    p.v1 = version == 1;
    let tensor_count = p.count()?;
    if tensor_count > MAX_TENSORS {
        return Err(GgufError::Corrupt(format!("{tensor_count} tensors")));
    }
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
        let v = p.value(ty, true, 0)?;
        metadata.insert(key, v);
    }
    Ok(GgufHeader { version, tensor_count, metadata })
}

struct Parser<R> {
    r: R,
    len: u64,
    /// Bytes consumed so far (tracked, so no seek syscalls are needed).
    pos: u64,
    v1: bool,
}

impl<R: Read + Seek> Parser<R> {
    /// Fails (before anything is allocated or read) if `n` more bytes would
    /// run past the end of the file or past [`MAX_HEADER_BYTES`].
    fn ensure(&self, n: u64) -> Result<(), GgufError> {
        let end = self.pos.checked_add(n).ok_or_else(|| GgufError::Corrupt("length overflow".into()))?;
        if end > self.len {
            return Err(GgufError::Corrupt(format!("truncated: need {n} bytes at offset {}", self.pos)));
        }
        if end > MAX_HEADER_BYTES {
            return Err(GgufError::Corrupt(format!("metadata larger than {MAX_HEADER_BYTES} bytes")));
        }
        Ok(())
    }
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], GgufError> {
        self.ensure(N as u64)?;
        let mut b = [0u8; N];
        self.r.read_exact(&mut b)?;
        self.pos += N as u64;
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
        if self.v1 { Ok(self.u32()? as u64) } else { self.u64() }
    }
    fn skip(&mut self, n: u64) -> Result<(), GgufError> {
        self.ensure(n)?;
        if n <= 64 * 1024 {
            // Small skips (vocab strings) read through the buffer: seeking a
            // BufReader drops its buffer and costs a syscall per token.
            let copied = std::io::copy(&mut (&mut self.r).take(n), &mut std::io::sink())?;
            if copied != n {
                return Err(GgufError::Corrupt("unexpected end of file".into()));
            }
        } else {
            // n <= MAX_HEADER_BYTES here, so the cast can't overflow.
            self.r.seek(SeekFrom::Current(n as i64))?;
        }
        self.pos += n;
        Ok(())
    }
    fn string(&mut self) -> Result<Value, GgufError> {
        let n = self.count()?;
        if n > MAX_STRING {
            self.skip(n)?;
            return Ok(Value::Skipped { len: n });
        }
        self.ensure(n)?; // before allocating
        let mut b = vec![0u8; n as usize];
        self.r.read_exact(&mut b)?;
        self.pos += n;
        Ok(Value::Str(String::from_utf8_lossy(&b).into_owned()))
    }
    fn value(&mut self, ty: u32, keep: bool, depth: u32) -> Result<Value, GgufError> {
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
                if depth >= MAX_ARRAY_DEPTH {
                    return Err(GgufError::Corrupt(format!("arrays nested deeper than {MAX_ARRAY_DEPTH}")));
                }
                let ety = self.u32()?;
                if ety > 12 {
                    return Err(GgufError::Corrupt(format!("unknown array element type {ety}")));
                }
                let n = self.count()?;
                let keep_items = keep && n <= MAX_ARRAY_KEEP;
                if !keep_items {
                    if let Some(sz) = fixed_size(ety) {
                        let total = n.checked_mul(sz).ok_or_else(|| GgufError::Corrupt(format!("array of {n} items")))?;
                        self.skip(total)?;
                    } else {
                        // Each variable-size item consumes >= 8 bytes, so
                        // `ensure` stops a bogus count at the header cap.
                        for _ in 0..n {
                            self.value(ety, false, depth + 1)?;
                        }
                    }
                    return Ok(Value::Skipped { len: n });
                }
                let mut items = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    items.push(self.value(ety, true, depth + 1)?);
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
        assert_eq!(h.metadata["tokenizer.ggml.tokens"], Value::Skipped { len: 5000 });
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
        assert_eq!(quant_from_filename("dolphin3.0-llama3.2-3b-q4_k_m.gguf").as_deref(), Some("Q4_K_M"));
        assert_eq!(quant_from_filename("Qwen2.5-0.5B-Instruct-Q8_0.gguf").as_deref(), Some("Q8_0"));
        assert_eq!(quant_from_filename("phi-3-mini-4k-instruct-IQ4_XS.gguf").as_deref(), Some("IQ4_XS"));
        assert_eq!(quant_from_filename("model-f16.gguf").as_deref(), Some("F16"));
        assert_eq!(quant_from_filename("model.gguf"), None);
    }

    // ---- hostile / edge-case headers, built from bytes ----

    struct B(Vec<u8>);
    impl B {
        fn new(version: u32, tensors: u64, kvs: u64) -> Self {
            let mut b = B(b"GGUF".to_vec());
            b.u32(version).u64(tensors).u64(kvs);
            b
        }
        fn u32(&mut self, v: u32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn u64(&mut self, v: u64) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn raw(&mut self, v: &[u8]) -> &mut Self {
            self.0.extend_from_slice(v);
            self
        }
        fn s(&mut self, v: &str) -> &mut Self {
            self.u64(v.len() as u64).raw(v.as_bytes())
        }
        fn parse(&self) -> Result<GgufHeader, GgufError> {
            read_header_from(Cursor::new(self.0.clone()), self.0.len() as u64)
        }
    }

    fn corrupt(r: Result<GgufHeader, GgufError>, needle: &str) {
        match r {
            Err(GgufError::Corrupt(m)) => assert!(m.contains(needle), "{m} !~ {needle}"),
            other => panic!("expected Corrupt({needle}), got {other:?}"),
        }
    }

    #[test]
    fn magic_and_version() {
        assert!(matches!(B(b"GGML\x03\0\0\0".to_vec()).parse(), Err(GgufError::BadMagic(_))));
        assert!(matches!(B(b"gguf".to_vec()).parse(), Err(GgufError::BadMagic(_))));
        assert!(matches!(B::new(0, 0, 0).parse(), Err(GgufError::Version(0))));
        assert!(matches!(B::new(4, 0, 0).parse(), Err(GgufError::Version(4))));
        assert!(matches!(B::new(u32::MAX, 0, 0).parse(), Err(GgufError::Version(_))));
        // Big-endian v3: version bytes 00 00 00 03.
        corrupt(B::new(3u32.swap_bytes(), 0, 0).parse(), "big-endian");
        // Zero-byte and tiny inputs.
        assert!(B(vec![]).parse().is_err());
        assert!(B(b"GG".to_vec()).parse().is_err());
        assert!(B(b"GGUF\x03\0".to_vec()).parse().is_err());
        // Minimal valid v3 and v2 headers with no metadata.
        assert_eq!(B::new(3, 0, 0).parse().unwrap().metadata.len(), 0);
        assert_eq!(B::new(2, 5, 0).parse().unwrap().tensor_count, 5);
    }

    #[test]
    fn v1_uses_u32_counts() {
        let mut b = B(b"GGUF".to_vec());
        b.u32(1).u32(2).u32(1); // v1, 2 tensors, 1 kv
        b.u32(4).raw(b"name").u32(8).u32(3).raw(b"abc");
        let h = b.parse().unwrap();
        assert_eq!((h.version, h.tensor_count, h.get_str("name")), (1, 2, Some("abc")));
    }

    #[test]
    fn absurd_counts_are_capped_without_allocating() {
        corrupt(B::new(3, u64::MAX, 0).parse(), "tensors");
        corrupt(B::new(3, 0, u64::MAX).parse(), "metadata entries");
        corrupt(B::new(3, 0, MAX_KV + 1).parse(), "metadata entries");
        // kv_count within the cap but the file ends: truncated, not a hang.
        corrupt(B::new(3, 0, MAX_KV).parse(), "truncated");
    }

    #[test]
    fn bad_string_lengths() {
        // Key length far beyond the file.
        let mut b = B::new(3, 0, 1);
        b.u64(u64::MAX);
        assert!(matches!(b.parse(), Err(GgufError::Corrupt(_))));
        let mut b = B::new(3, 0, 1);
        b.u64(1000).raw(b"short");
        corrupt(b.parse(), "truncated");
        // Value string length beyond the file (kept-size path: no 200 KB alloc).
        let mut b = B::new(3, 0, 1);
        b.s("k").u32(8).u64(200 * 1024).raw(b"x");
        corrupt(b.parse(), "truncated");
        // Oversized (skipped) string past the end.
        let mut b = B::new(3, 0, 1);
        b.s("k").u32(8).u64(10 * 1024 * 1024);
        corrupt(b.parse(), "truncated");
        // Oversized key is refused.
        let big = "k".repeat(MAX_STRING as usize + 1);
        let mut b = B::new(3, 0, 1);
        b.s(&big).u32(4).u32(1);
        corrupt(b.parse(), "oversized key");
    }

    #[test]
    fn invalid_utf8_is_lossy_not_an_error() {
        let mut b = B::new(3, 0, 2);
        b.u64(3).raw(b"k\xff1").u32(8).u64(2).raw(b"\xc3\x28");
        b.s("general.name").u32(8).u64(4).raw(b"ok\xe2\x82");
        let h = b.parse().unwrap();
        assert_eq!(h.metadata.len(), 2);
        assert!(h.name().unwrap().starts_with("ok"));
    }

    #[test]
    fn unknown_value_types() {
        for t in [13u32, 99, u32::MAX] {
            let mut b = B::new(3, 0, 1);
            b.s("k").u32(t).u64(0);
            corrupt(b.parse(), "unknown value type");
        }
        let mut b = B::new(3, 0, 1);
        b.s("k").u32(9).u32(77).u64(1);
        corrupt(b.parse(), "unknown array element type");
    }

    #[test]
    fn arrays_nested_and_bounded() {
        // [[1,2],[3]] of u8: kept.
        let mut b = B::new(3, 0, 1);
        b.s("nested").u32(9).u32(9).u64(2);
        b.u32(0).u64(2).raw(&[1, 2]);
        b.u32(0).u64(1).raw(&[3]);
        let h = b.parse().unwrap();
        assert_eq!(
            h.metadata["nested"],
            Value::Array(vec![Value::Array(vec![Value::U64(1), Value::U64(2)]), Value::Array(vec![Value::U64(3)])])
        );
        // Deep nesting (stack-overflow bait) is refused.
        let mut b = B::new(3, 0, 1);
        b.s("deep").u32(9);
        for _ in 0..10_000 {
            b.u32(9).u64(1);
        }
        corrupt(b.parse(), "nested deeper");
        // Huge fixed-size array: count * size overflows / runs past the end.
        let mut b = B::new(3, 0, 1);
        b.s("a").u32(9).u32(12).u64(u64::MAX / 4);
        assert!(matches!(b.parse(), Err(GgufError::Corrupt(_))));
        let mut b = B::new(3, 0, 1);
        b.s("a").u32(9).u32(6).u64(1_000_000);
        corrupt(b.parse(), "truncated");
        // Huge string-array count with too few bytes: stops at EOF.
        let mut b = B::new(3, 0, 1);
        b.s("a").u32(9).u32(8).u64(u64::MAX);
        for _ in 0..100 {
            b.s("t");
        }
        corrupt(b.parse(), "truncated");
        // Array of unknown nested element type inside a skipped array.
        let mut b = B::new(3, 0, 1);
        b.s("a").u32(9).u32(9).u64(100);
        b.u32(200).u64(0);
        assert!(b.parse().is_err());
    }

    #[test]
    fn header_size_is_capped() {
        // A "valid" header whose metadata would extend past MAX_HEADER_BYTES
        // is refused before reading it (len claims a huge file).
        struct Zeros(u64);
        impl Read for Zeros {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                buf.fill(0);
                self.0 += buf.len() as u64;
                Ok(buf.len())
            }
        }
        impl Seek for Zeros {
            fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
                if let SeekFrom::Current(d) = p {
                    self.0 = (self.0 as i64 + d) as u64;
                }
                Ok(self.0)
            }
        }
        let mut head = B::new(3, 0, 1);
        head.s("big").u32(9).u32(0).u64(MAX_HEADER_BYTES); // u8 array of 64 MiB
        let r = std::io::Cursor::new(head.0.clone()).chain(Zeros(0));
        struct Both<A, Bb>(A, Bb);
        impl<A: Read, Bb> Read for Both<A, Bb> {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                self.0.read(b)
            }
        }
        impl<A, Bb: Seek> Seek for Both<A, Bb> {
            fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
                self.1.seek(p)
            }
        }
        let e = read_header_from(Both(r, Zeros(0)), 8 * 1024 * 1024 * 1024).unwrap_err();
        assert!(e.to_string().contains("larger than"), "{e}");
    }

    #[test]
    fn files_dirs_symlinks_and_specials() {
        let dir = std::env::temp_dir().join(format!("lillith-gguf-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("m.gguf");
        std::fs::write(&good, sample()).unwrap();
        assert!(read_header(&good).is_ok());
        assert!(is_gguf(&good));
        // Zero-byte file.
        let empty = dir.join("empty.gguf");
        std::fs::write(&empty, b"").unwrap();
        assert!(read_header(&empty).is_err());
        assert!(!is_gguf(&empty));
        // Directory named *.gguf.
        let d = dir.join("dir.gguf");
        std::fs::create_dir_all(&d).unwrap();
        assert!(read_header(&d).is_err());
        assert!(!is_gguf(&d));
        // Symlink to a good file works; dangling symlink is an error.
        let link = dir.join("link.gguf");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert!(read_header(&link).is_ok());
        let dangling = dir.join("dangling.gguf");
        std::os::unix::fs::symlink(dir.join("nope"), &dangling).unwrap();
        assert!(read_header(&dangling).is_err());
        // A FIFO must be refused without blocking on open().
        let fifo = dir.join("fifo.gguf");
        if std::process::Command::new("mkfifo").arg(&fifo).status().is_ok_and(|s| s.success()) {
            assert!(read_header(&fifo).is_err());
            assert!(!is_gguf(&fifo));
        }
        // /dev/zero: a character device, never read.
        assert!(read_header(Path::new("/dev/zero")).is_err());
        // Unreadable file (skipped when running as root, which ignores modes).
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.join("locked.gguf");
        std::fs::write(&locked, sample()).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&locked).is_err() {
            assert!(matches!(read_header(&locked), Err(GgufError::Io(_))));
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncation_at_every_offset_never_panics() {
        let full = sample();
        for cut in 0..full.len() {
            let r = read_header_from(Cursor::new(full[..cut].to_vec()), cut as u64);
            assert!(r.is_err(), "cut at {cut} parsed");
        }
        // And a length that lies (claims more than the reader has).
        assert!(read_header_from(Cursor::new(full[..40].to_vec()), full.len() as u64).is_err());
    }

    #[test]
    fn random_garbage_never_panics() {
        // Deterministic xorshift fuzz over mutated headers.
        let base = sample();
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..3000 {
            let mut b = base.clone();
            for _ in 0..4 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let i = (x as usize) % b.len().min(200);
                b[i] = (x >> 32) as u8;
            }
            let len = b.len() as u64;
            let _ = read_header_from(Cursor::new(b), len);
        }
    }
}
