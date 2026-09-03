mod reader;
mod varint;
mod writer;

pub use reader::*;
pub use varint::*;
pub use writer::*;

pub trait Writable {
    fn encode(&self, writer: &mut Writer);
}

pub trait Readable: Sized {
    type Error;
    fn decode(reader: &mut Reader) -> Result<Self, Self::Error>;
}
