use thiserror::Error;

use crate::{
    codec::{ReadError, Reader, Writer},
    hlc::Timestamp,
    uuid::Uuid,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct OpHeader {
    /// The **server-attested** user id for this entry. Every entry originates
    /// from *some* user, but this field specifically carries the identity a
    /// server vouched for — it is populated only in server-mode segments, where
    /// the server asserts attribution. `None` in device mode, where attribution
    /// is implicit from the peer directory and no server assertion exists.
    ///
    /// Do not read this as "the author"; read it as "who the server said this
    /// was." It is distinct from a stream's `peer_id` (which stream the entry
    /// came from).
    ///
    /// This _can_ be empty in server logs if and only if none of the ops are user attributable.
    pub server_attested_user_id: Option<Uuid>,
    /// HLC timestamp — monotonically non-decreasing within a peer's stream.
    /// Entries written in one atomic transaction share a tick, so they are
    /// treated as one logical write by LWW comparisons.
    pub timestamp: Timestamp,
}

#[derive(Error, Debug)]
pub enum HeaderDecodeError {
    #[error("read error: {0}")]
    ReadError(#[from] ReadError),
    #[error("invalid server attested user id length: {length}")]
    InvalidServerAttestedId { length: usize },
}

impl OpHeader {
    /// Decode the header from a buffer which contains ONLY header bytes.
    /// IMPORTANT: This method assumes the header bytes were ALREADY read with a length-delimited prefix!
    pub fn decode(buf: &[u8]) -> Result<Self, HeaderDecodeError> {
        let mut reader = Reader::new(buf);
        let timestamp = Timestamp::from_raw(reader.read_le_u64()?);
        // NOTE: all the remaining header bytes are for the server user id which is len delimited at the layer above this
        let server_user_id_bytes = reader.into_remaining();
        let n = server_user_id_bytes.len();
        if n == 0 {
            Ok(OpHeader {
                server_attested_user_id: None,
                timestamp,
            })
        } else if n != 16 {
            // for now only handle UUIDs
            Err(HeaderDecodeError::InvalidServerAttestedId { length: n })
        } else {
            Ok(OpHeader {
                server_attested_user_id: Some(server_user_id_bytes.try_into().unwrap()), // length already checked above
                timestamp,
            })
        }
    }

    pub fn encode(&self, writer: &mut Writer) {
        writer.write_var_usize(self.encoded_len());
        self.encode_body(writer);
    }

    /// Returns the encoded length of the header in its canonical form.
    /// IMPORTANT: when encoding, the encoder must first prepend the encoded length and then encode.
    pub fn encoded_len(&self) -> usize {
        if self.server_attested_user_id.is_some() {
            8 + 16 // ts + uuid
        } else {
            8 // just ts
        }
    }

    /// Encodes the body header to the writer.
    /// IMPORTANT: when encoding, the encoder must first prepend the encoded length to the output and then encode the body.
    pub fn encode_body(&self, writer: &mut Writer) {
        writer.write_le_u64(self.timestamp.raw());
        if let Some(id) = self.server_attested_user_id {
            writer.write_array(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use test_strategy::proptest;

    use crate::{
        codec::{Reader, Writer},
        log::OpHeader,
    };

    #[proptest]
    fn test_roundtrip(header: OpHeader) {
        let mut w = Writer::new();
        header.encode(&mut w);
        let res = w.finalize();

        let mut r = Reader::new(&res);
        let len = r.read_var_usize().unwrap();
        let rest = r.into_remaining();
        assert_eq!(len, rest.len());
        let header2 = OpHeader::decode(rest).unwrap();
        assert_eq!(header, header2);
    }
}
