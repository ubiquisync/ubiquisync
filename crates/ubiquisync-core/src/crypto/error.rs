use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("cipher error")]
    CipherError,
    #[error("op count range exceeded")]
    OutOfRangeOp,
}
