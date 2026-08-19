use std::ops::Range;

use crate::{
    codec::reader::Reader,
    init::Version,
    log_entry::{CipherInfo, GenericLogEntry, LogDecodeError, OpaqueLogEntry},
};

pub struct SegmentMeta {
    pub range: Range<u64>,
}

pub struct SegmentHeader {
    pub version: Version,
    pub encoding: SegmentEncoding,
}

pub enum SegmentEncoding {
    Opaque,
    Plaintext(PlaintextSegmentEncoding),
}

pub struct PlaintextSegmentEncoding {
    pub outer_encryption: CipherInfo,
    pub inner_compression: Compression,
}

pub enum Compression {
    Zstd = 0,
}

pub enum SegmentDecodeError {}

fn decode_opaque_segment<'a>(
    meta: &SegmentMeta,
    reader: &mut Reader<'a>,
) -> Result<Vec<OpaqueLogEntry<'a>>, LogDecodeError> {
    let mut next_entry_index = meta.range.start;
    let mut entries = vec![];
    while !reader.is_empty() {
        let entry = GenericLogEntry::decode(reader, next_entry_index)?;
        entries.push(entry.clone());
        if let Some(idx) = entry.next_entry_index() {
            next_entry_index = idx;
        }
    }
    // TODO check end index
    Ok(entries)
}
