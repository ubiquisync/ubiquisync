use thiserror::Error;

use crate::codec::reader::ReadError;

#[derive(Error, Debug)]
pub enum EncodeError {
    #[error("empty op")]
    EmptyOp,
}

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("read error: {0}")]
    ReadError(#[from] ReadError),
}
