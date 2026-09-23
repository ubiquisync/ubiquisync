use std::ops::Range;

use thiserror::Error;

use crate::codec::varint::{decode_var_u64, decode_zigzag_i64};

pub struct Reader<'a> {
    buf: &'a [u8],
}

// TODO rename to DataReadError
#[derive(Error, Debug)]
pub enum ReadError {
    #[error("unexpected EOF")]
    UnexpectedEof,
    #[error("non-minimal varint")]
    NonMinimalVarint,
    #[error("usize overflow: {0}")]
    USizeOverflow(u64),
    #[error("invalid range with start {start} and span {span}")]
    InvalidRange { start: u64, span: u64 },
    #[error("trailing bytes")]
    TrailingBytes,
    #[error("error reading Option<T>")]
    InvalidOption,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf }
    }

    pub fn read_byte(&mut self) -> Result<u8, ReadError> {
        let (x, rest) = self.buf.split_first().ok_or(ReadError::UnexpectedEof)?;
        self.buf = rest;
        Ok(*x)
    }

    pub fn read_slice(&mut self, n: usize) -> Result<&'a [u8], ReadError> {
        let (bz, rest) = self
            .buf
            .split_at_checked(n)
            .ok_or(ReadError::UnexpectedEof)?;
        self.buf = rest;
        Ok(bz)
    }

    pub fn read_len_prefixed(&mut self) -> Result<&'a [u8], ReadError> {
        let n = self.read_var_usize()?;
        self.read_slice(n)
    }

    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
        Ok(self.read_slice(N)?.try_into().unwrap())
    }

    pub fn read_var_u64(&mut self) -> Result<u64, ReadError> {
        let (x, rest) = decode_var_u64(self.buf)?;
        self.buf = rest;
        Ok(x)
    }

    /// Reads a usize as a var u64 with a checked conversion for overflow.
    pub fn read_var_usize(&mut self) -> Result<usize, ReadError> {
        let n = self.read_var_u64()?;
        n.try_into().map_err(|_| ReadError::USizeOverflow(n))
    }

    pub fn read_le_u64(&mut self) -> Result<u64, ReadError> {
        Ok(u64::from_le_bytes(self.read_array()?))
    }

    pub fn read_le_u16(&mut self) -> Result<u16, ReadError> {
        Ok(u16::from_le_bytes(self.read_array()?))
    }

    pub fn read_zigzag_i64(&mut self) -> Result<i64, ReadError> {
        let (x, rest) = decode_zigzag_i64(self.buf)?;
        self.buf = rest;
        Ok(x)
    }

    pub fn read_range(&mut self) -> Result<Range<u64>, ReadError> {
        let start = self.read_var_u64()?;
        let span = self.read_var_u64()?;
        if span == 0 {
            return Err(ReadError::InvalidRange { start, span });
        }
        let end = start
            .checked_add(span)
            .ok_or(ReadError::InvalidRange { start, span })?;
        Ok(Range { start, end })
    }

    pub fn read_option<T, F, E>(&mut self, mut on_some: F) -> Result<Option<T>, E>
    where
        F: FnMut(&mut Self) -> Result<T, E>,
        E: From<ReadError>,
    {
        match self.read_byte()? {
            0 => Ok(None),
            1 => Ok(Some(on_some(self)?)),
            _ => Err(ReadError::InvalidOption.into()),
        }
    }

    pub fn read_vec<T, F, E>(&mut self, mut decode: F) -> Result<Vec<T>, E>
    where
        F: FnMut(&mut Self) -> Result<T, E>,
        E: From<ReadError>,
    {
        let n = self.read_var_usize()?;
        // NOTE: DO NOT pre-allocate the vec, input could be untrusted and cause OOM
        let mut res = vec![];
        for _ in 0..n {
            res.push(decode(self)?);
        }
        Ok(res)
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn remaining(&self) -> &'a [u8] {
        self.buf
    }

    pub fn into_remaining(self) -> &'a [u8] {
        self.buf
    }

    pub fn require_empty(&self) -> Result<(), ReadError> {
        if !self.is_empty() {
            return Err(ReadError::TrailingBytes);
        }
        Ok(())
    }
}
