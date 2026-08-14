use crate::codec::varint::{
    MAX_VAR_U64_SIZE, MAX_ZIGZAG_I64_SIZE, encode_var_u64, encode_zigzag_i64,
};

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn write_byte(&mut self, x: u8) {
        self.buf.push(x)
    }

    pub fn write_var_u64(&mut self, x: u64) {
        let mut buf = [0; MAX_VAR_U64_SIZE];
        let res = encode_var_u64(x, &mut buf);
        self.write_bytes(res)
    }

    pub fn write_var_usize(&mut self, x: usize) {
        self.write_var_u64(x as u64)
    }

    pub fn write_le_u64(&mut self, x: u64) {
        self.write_bytes(&x.to_le_bytes()[..])
    }

    pub fn write_zigzag_i64(&mut self, x: i64) {
        let mut buf = [0; MAX_ZIGZAG_I64_SIZE];
        let res = encode_zigzag_i64(x, &mut buf);
        self.write_bytes(res)
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
    fn test_roundtrip(a: Vec<u8>, b: u8, c: u64, d: u64, e: Vec<u8>, f: i64) {
        let mut w = Writer::new();

        w.write_var_usize(a.len());
        w.write_bytes(&a);
        w.write_byte(b);
        w.write_var_u64(c);
        w.write_le_u64(d);
        w.write_var_usize(e.len());
        w.write_bytes(&e);
        w.write_zigzag_i64(f);

        let res = w.finalize();
        let mut r = Reader::new(&res);

        let a_len = r.read_var_usize().unwrap();
        assert_eq!(a.len(), a_len);
        assert_eq!(a.as_slice(), r.read_slice(a_len).unwrap());
        assert_eq!(b, r.read_byte().unwrap());
        assert_eq!(c, r.read_var_u64().unwrap());
        assert_eq!(d, r.read_le_u64().unwrap());
        let e_len = r.read_var_usize().unwrap();
        assert_eq!(e.len(), e_len);
        assert_eq!(e.as_slice(), r.read_slice(e_len).unwrap());
        assert_eq!(f, r.read_zigzag_i64().unwrap());

        assert_eq!(0, r.unwrap().len())
    }
}
