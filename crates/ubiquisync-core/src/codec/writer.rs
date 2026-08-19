use std::ops::Range;

use thiserror::Error;

use crate::codec::varint::{
    MAX_VAR_U64_SIZE, MAX_ZIGZAG_I64_SIZE, encode_var_u64, encode_zigzag_i64,
};

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

#[derive(Error, Debug)]
pub enum WriteError {
    #[error("invalid range {0:?}")]
    EmptyRange(Range<u64>),
}

impl Writer {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn write_slice(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn write_array<const N: usize>(&mut self, array: &[u8; N]) {
        self.buf.extend_from_slice(&array[..]);
    }

    pub fn write_len_prefixed(&mut self, bytes: &[u8]) {
        self.write_var_u64(bytes.len() as u64);
        self.write_slice(bytes);
    }

    pub fn write_byte(&mut self, x: u8) {
        self.buf.push(x)
    }

    pub fn write_var_u64(&mut self, x: u64) {
        let mut buf = [0; MAX_VAR_U64_SIZE];
        let res = encode_var_u64(x, &mut buf);
        self.write_slice(res)
    }

    /// Write a usize as a var u64 (exists to match read_var_usize).
    pub fn write_var_usize(&mut self, x: usize) {
        self.write_var_u64(x as u64)
    }

    pub fn write_le_u64(&mut self, x: u64) {
        self.write_slice(&x.to_le_bytes()[..])
    }

    pub fn write_zigzag_i64(&mut self, x: i64) {
        let mut buf = [0; MAX_ZIGZAG_I64_SIZE];
        let res = encode_zigzag_i64(x, &mut buf);
        self.write_slice(res)
    }

    pub fn write_range(&mut self, range: &Range<u64>) -> Result<(), WriteError> {
        if range.is_empty() {
            return Err(WriteError::EmptyRange(range.clone()));
        }
        self.write_var_u64(range.start);
        let span = range.end - range.start; // already checked with range.is_empty()
        self.write_var_u64(span);
        Ok(())
    }

    pub fn finalize(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use test_strategy::proptest;

    use crate::codec::{reader::Reader, writer::Writer};

    // Round trips some random data with all encodings Reader & Writer support.
    #[proptest]
    fn test_roundtrip(a: Vec<u8>, b: u8, c: u64, d: u64, e: Vec<u8>, f: i64, g: [u8; 16]) {
        let mut w = Writer::new();

        w.write_len_prefixed(&a);
        w.write_byte(b);
        w.write_var_u64(c);
        w.write_le_u64(d);
        w.write_len_prefixed(&e);
        w.write_zigzag_i64(f);
        w.write_array(&g);

        let res = w.finalize();
        let mut r = Reader::new(&res);

        assert_eq!(a.as_slice(), r.read_len_prefixed().unwrap());
        assert_eq!(b, r.read_byte().unwrap());
        assert_eq!(c, r.read_var_u64().unwrap());
        assert_eq!(d, r.read_le_u64().unwrap());
        assert_eq!(e.as_slice(), r.read_len_prefixed().unwrap());
        assert_eq!(f, r.read_zigzag_i64().unwrap());
        assert_eq!(g, r.read_array::<16>().unwrap());

        assert!(r.is_empty());
        assert_eq!(0, r.remaining().len());
        assert_eq!(0, r.into_remaining().len())
    }

    // TODO test the failure path
}
