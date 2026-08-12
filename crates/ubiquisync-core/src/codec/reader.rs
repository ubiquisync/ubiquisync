use thiserror::Error;

use crate::codec::varint::{VarintDecodeError, decode_var_u64};

pub struct Reader<'a> {
    buf: &'a [u8],
}

#[derive(Error, Debug)]
pub enum ReadError {
    #[error("unexpected EOF")]
    UnexpectedEof,
    #[error("varint decode error: {0}")]
    VarintDecodeError(#[from] VarintDecodeError),
    #[error("overflow")]
    Overflow,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf }
    }

    pub fn read_byte(&mut self) -> Result<u8, ReadError> {
        if self.buf.is_empty() {
            return Err(ReadError::UnexpectedEof);
        }

        let x = self.buf[0];
        self.buf = &self.buf[1..];
        Ok(x)
    }

    pub fn read_slice(&mut self, n: usize) -> Result<&'a [u8], ReadError> {
        if self.buf.len() < n {
            return Err(ReadError::UnexpectedEof);
        }

        let bytes = &self.buf[..n];
        self.buf = &self.buf[n..];
        Ok(bytes)
    }

    pub fn read_var_u64(&mut self) -> Result<u64, ReadError> {
        let (x, rest) = decode_var_u64(self.buf)?;

        self.buf = rest;
        Ok(x)
    }

    pub fn read_var_usize(&mut self) -> Result<usize, ReadError> {
        let x = self.read_var_u64()?;
        let x: usize = x.try_into().map_err(|_| ReadError::Overflow)?;
        Ok(x)
    }

    pub fn reader_le_u64(&mut self) -> Result<u64, ReadError> {
        Ok(u64::from_le_bytes(self.read_slice(8)?.try_into().unwrap()))
    }

    pub fn unwrap(self) -> &'a [u8] {
        self.buf
    }
}
