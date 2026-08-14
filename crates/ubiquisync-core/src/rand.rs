use thiserror::Error;

pub fn rand_fill(buf: &mut [u8]) -> Result<(), Error> {
    getrandom::fill(buf).map_err(|_| Error)
}

#[derive(Error, Debug)]
#[error("error generating random numbers")]
pub struct Error;
