use alloc::borrow::Borrow;

use crate::{
    bytes::{OpaqueBytes, PlaintextBytes},
    codec::{Reader, Writer},
    crypto::{CipherInfo, Hash256, Signature},
    log::{LogDecodeError, LogEncodeError, OpBatch},
};

/// Represents a single entry in a stream of logs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum LogEntry<B: std::fmt::Debug> {
    IndexedEntry(EntryBody<B>),
    Signature(Signature),
    // TODO we could consider adding some explicit forward-compatible support for unknown entries
    // where if an entry type byte has specific flags set we can hash and encrypt it and verify
    // signatures on top of it without actually being able to process it.
    // Should get addressed pre-v1.
}

/// Log entry where op and header are encoded as canonical hash bytes (may be encrypted)
pub type OpaqueLogEntry<'a> = LogEntry<OpaqueBytes<'a>>;

pub type PlaintextLogEntry<'a> = LogEntry<PlaintextBytes<'a>>;

/// The content of signed and indexed log entries.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum EntryBody<B: std::fmt::Debug> {
    /// An operation batch in the app's op vocabulary.
    OpBatch(OpBatch<B>),
    /// Declares the fingerprint for the encryption key being used from
    /// this point forward until the next UseKey op changes the key.
    ///
    /// MUST NOT be expunged.
    UseKey(CipherInfo),
    /// Expunges a single entry.
    ///
    /// Expunging entries SHOULD be used extremely rarely as a last line of defense.
    /// It is included primarily to ensure there is a exit hatch for deleting just one thing
    /// when needed for compliance or privacy reasons.
    /// Expunging entries may make the log stream basically unusable if not done correctly.
    /// For instance, it is an irrecoverable error to expunge a `UseKey` entry because this
    /// will make all following entries indecipherable.
    /// Different op vocabs may fail in similarly spectacular ways - expunging an op that
    /// has some downstream dependency (such as a YDoc update that other updates depend on)
    /// may result essential "bricked" state.
    /// For this reason, expunging an entry should be gated as an admin-only operation wherever
    /// possible. It SHOULD NOT be considered trivially safe to let client's expunge their own
    /// entries - having authored an entry does not absolve a peer of the dependency other peers
    /// may have placed on that entry. To allow "full-deletion" of user content, apps should
    /// generally prefer another cheaper mechanism which is to make a "log container" the unit
    /// of deletable content. For instance, if a document is represented by a single log container,
    /// it is much cheaper to delete the whole bundle of logs associated with that document rather
    /// than expunge individual entries.
    /// Exceptions can be made by op vocabularies where it is safe - for instance, it generally wouldj
    /// be safe to to allow users to expunge individual entries from a chat thread and this would
    /// be the preferred behavior for such content. Even though other chat entries do reference
    /// one another, _usually_ the entries do not introduce causal dependencies in the way a
    /// document CRDT would. So a chat op vocabulary, _could_ choose to make expunging a routine
    /// mechanism for deleting individual user entries for that vocabulary.
    /// So the important operative is to use with care.
    /// Whenever possible, log processors should never accept expunged entry from untrusted peers
    /// without some sort of signed authorization proving that the expunge operation is permitted.
    /// Expunging could be abused as a mechanism for censoring data from specific peers.
    /// So generally, expunge must be used with per op-vocabulary rules, and all peers should know
    /// the rules and abide by them.
    Expunged(Hash256),
}

impl<B: alloc::fmt::Debug> LogEntry<B> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), LogEncodeError>
    where
        B: Borrow<[u8]>,
    {
        match self {
            LogEntry::IndexedEntry(entry) => match entry {
                EntryBody::OpBatch(op_batch) => {
                    writer.write_byte(ENTRY_TYPE_OP_BATCH);
                    op_batch.encode(writer)?;
                }
                EntryBody::UseKey(cipher_info) => {
                    writer.write_byte(ENTRY_TYPE_USE_KEY);
                    cipher_info.encode(writer);
                }
                EntryBody::Expunged(hash) => {
                    writer.write_byte(ENTRY_TYPE_EXPUNGED);
                    writer.write_array(hash);
                }
            },
            LogEntry::Signature(signature) => {
                // NOTE: size is inferred by the last entry, it's the callers responsibility to verify before encoding
                writer.write_byte(ENTRY_TYPE_SIGNATURE);
                signature.encode(writer);
            }
        }
        Ok(())
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, LogDecodeError>
    where
        B: From<&'a [u8]>,
    {
        let entry_type = reader.read_byte()?;
        Ok(match entry_type {
            ENTRY_TYPE_OP_BATCH => Self::IndexedEntry(
                // TODO max op length
                EntryBody::OpBatch(OpBatch::decode(reader)?),
            ),
            ENTRY_TYPE_SIGNATURE => Self::Signature(
                Signature::decode(reader).map_err(LogDecodeError::from_sig_decode_err)?,
            ),
            ENTRY_TYPE_USE_KEY => {
                Self::IndexedEntry(EntryBody::UseKey(CipherInfo::decode(reader)?))
            }
            ENTRY_TYPE_EXPUNGED => Self::IndexedEntry(EntryBody::Expunged(reader.read_array()?)),
            unknown => {
                return Err(LogDecodeError::UndecodableEntryType(unknown));
            }
        })
    }
}

const ENTRY_TYPE_OP_BATCH: u8 = 0x00;
const ENTRY_TYPE_USE_KEY: u8 = 0x01;
const ENTRY_TYPE_SIGNATURE: u8 = 0x02;
const ENTRY_TYPE_EXPUNGED: u8 = 0x03;

#[cfg(test)]
mod tests {
    use test_strategy::proptest;

    use crate::log::LogEntry;
    #[cfg(test)]
    use crate::{
        bytes::OpaqueBytes,
        codec::{Reader, Writer},
    };

    #[proptest]
    fn test_round_trip(entry: LogEntry<OpaqueBytes<'static>>) {
        let mut w = Writer::new();
        entry.encode(&mut w).unwrap();
        let res = w.finalize();

        let mut r = Reader::new(&res);
        let decoded = LogEntry::decode(&mut r).unwrap();
        assert_eq!(entry, decoded);
    }
}
