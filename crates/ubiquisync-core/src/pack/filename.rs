use std::{
    collections::{HashMap, hash_map::Entry},
    fmt::Display,
    ops::Range,
    path::PathBuf,
    str::FromStr,
};

use thiserror::Error;

use crate::{crypto::Hasher, ids::PeerId};

use crate::{
    codec::{ReadError, Reader, WriteError, Writer},
    pack::PackRef,
};

pub struct PackFileDescriptor {
    pub topic: Topic,
    pub peer_id: PeerId,
    pub id: PackFileId,
}

/// A topic if composed of one or more lowercase ASCII alphanumeric segments.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Topic(Vec<String>);

#[derive(Debug, Error)]
#[error("invalid characters in topic")]
pub struct TopicError;

impl Topic {
    pub fn new<S: AsRef<str>>(parts: &[S]) -> Result<Self, TopicError> {
        let res = Self(parts.iter().map(|s| s.as_ref().to_string()).collect());
        res.validate()?;
        Ok(res)
    }

    pub fn validate(&self) -> Result<(), TopicError> {
        for part in self.0.iter() {
            if part.is_empty()
                || !part
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
            {
                return Err(TopicError);
            }
        }
        Ok(())
    }

    pub fn dir(&self) -> String {
        if self.0.is_empty() {
            "_".into()
        } else {
            self.0
                .iter()
                .map(|p| format!("_{p}"))
                .collect::<Vec<_>>()
                .join("/")
        }
    }

    pub fn sub_topic(&self, part: &str) -> Result<Topic, TopicError> {
        let mut sub = self.clone();
        sub.0.push(part.into());
        sub.validate()?;
        Ok(sub)
    }

    pub fn peer_dir(&self, peer: &PeerId) -> String {
        format!("{}/{}", self.dir(), hex::encode(&peer.0))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub struct PackFileId {
    pub seqs: Range<u64>,
    pub id: u64,
    pub generation: u64,
}

impl PackFileId {
    pub fn get_ref(&self) -> PackRef {
        PackRef {
            id: self.id,
            end_seq: self.seqs.end,
        }
    }

    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        w.write_range(&self.seqs)?;
        // fixed length because id is random
        w.write_le_u64(self.id);
        w.write_var_u64(self.generation);
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, ReadError> {
        let seqs = r.read_range()?;
        let id = r.read_le_u64()?;
        let generation = r.read_var_u64()?;
        Ok(Self {
            seqs,
            id,
            generation,
        })
    }
}

impl Display for PackFileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:04x}-{:04x}-{:016x}-{:02x}",
            self.seqs.start, self.seqs.end, self.id, self.generation
        )
    }
}

#[derive(Debug, Error)]
#[error("error parsing filename")]
pub struct ParseFileNameError;

impl FromStr for PackFileId {
    type Err = ParseFileNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('-');
        let mut parse_part = || {
            parts
                .next()
                .and_then(|p| u64::from_str_radix(p, 16).ok())
                .ok_or(ParseFileNameError)
        };

        let start = parse_part()?;
        let end = parse_part()?;
        let id = parse_part()?;
        let generation = parse_part()?;

        // ensure no trailing chars and range is valid
        if parts.next().is_some() || start >= end {
            return Err(ParseFileNameError);
        }

        let id = Self {
            seqs: start..end,
            id,
            generation,
        };

        // ensure canonical form
        // this matters for roundtripping ID's to real file names for GC
        if id.to_string() != s {
            return Err(ParseFileNameError);
        }

        Ok(id)
    }
}

pub(crate) const HEADER_EXT: &str = ".upq.meta";
pub(crate) const BODY_EXT: &str = ".upq.meta";

impl PackFileDescriptor {
    pub(crate) fn hash(&self, hasher: &mut Hasher) -> Result<(), WriteError> {
        let mut w = Writer::new();
        self.id.encode(&mut w)?;
        let id_bytes = w.finalize();
        hasher.update(&id_bytes);
        hasher.update(&self.peer_id.0);
        hasher.update_varint(self.topic.0.len() as u64);
        for p in self.topic.0.iter() {
            hasher.update_len_prefixed(p.as_bytes());
        }
        Ok(())
    }

    pub(crate) fn header_path(&self) -> String {
        self.file_name(HEADER_EXT)
    }

    pub(crate) fn body_path(&self) -> String {
        self.file_name(BODY_EXT)
    }

    fn file_name(&self, ext: &str) -> String {
        format!(
            "{}/{}{ext}",
            self.topic.peer_dir(&self.peer_id),
            self.id.to_string()
        )
    }
}

/// Dedupes files and chooses the latest valid generation of a pack.
/// NOTE: if a generation file is corrupted garbage we could inspect
/// earlier generations at read time, but if this happens in a shared folder
/// there are bigger problems (noting this here so that reviewers don't keep
/// flagging a thread that doesn't match the threat model).
pub fn dedupe_pack_files(files: &[PackFileId]) -> Vec<PackFileId> {
    let mut files_by_ref = HashMap::<PackRef, PackFileId>::new();
    for f in files.iter() {
        match files_by_ref.entry(f.get_ref()) {
            Entry::Occupied(mut e) => {
                if e.get().generation < f.generation {
                    e.insert(f.clone());
                }
            }
            Entry::Vacant(e) => {
                e.insert(f.clone());
            }
        }
    }
    files_by_ref.into_values().collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {
    use std::{ops::Range, str::FromStr};

    use test_case::test_case;
    use test_strategy::proptest;

    use crate::pack::{PackFileId, dedupe_pack_files};

    #[test_case(0..1, 0xabcdef, 0 => "0000-0001-0000000000abcdef-00")]
    #[test_case(0xffff..0xa0000, 0x12345, 0x100 => "ffff-a0000-0000000000012345-100")]
    fn pack_file_id_display(seqs: Range<u64>, id: u64, generation: u64) -> String {
        PackFileId {
            seqs,
            id,
            generation,
        }
        .to_string()
    }

    #[proptest]
    fn pack_file_id_roundtrips(id: PackFileId) {
        let parsed = PackFileId::from_str(&id.to_string()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test_case("0000-0001-0000000000abcdef" ; "too few parts")]
    #[test_case("0000-0001-0000000000abcdef-00-00" ; "trailing part")]
    #[test_case("0005-0005-0000000000abcdef-00" ; "empty range")]
    #[test_case("0009-0002-0000000000abcdef-00" ; "backwards range")]
    #[test_case("0000-0001-0000000000ABCDEF-00" ; "uppercase")]
    #[test_case("00000-0001-0000000000abcdef-00" ; "extra zero")]
    #[test_case("+000-0001-0000000000abcdef-00" ; "leading plus")]
    fn pack_file_id_rejects(s: &str) {
        assert!(PackFileId::from_str(s).is_err());
    }

    #[test_case(&[] => Vec::<String>::new() ; "empty")]
    #[test_case(&["0000-000a-0000000000000001-00"]
        => vec!["0000-000a-0000000000000001-00"] ; "single")]
    #[test_case(&["0000-000a-0000000000000001-00", "0000-000a-0000000000000001-02"]
        => vec!["0000-000a-0000000000000001-02"] ; "higher gen wins")]
    #[test_case(&["0000-000a-0000000000000001-02", "0000-000a-0000000000000001-00"]
        => vec!["0000-000a-0000000000000001-02"] ; "higher gen wins regardless of order")]
    #[test_case(&["0005-000a-0000000000000001-00", "0000-000a-0000000000000001-01"]
        => vec!["0000-000a-0000000000000001-01"] ; "rewrite with wider range replaces")]
    #[test_case(&["0000-000a-0000000000000001-00", "0000-0014-0000000000000001-00"]
        => vec!["0000-000a-0000000000000001-00", "0000-0014-0000000000000001-00"] ; "same id different end kept")]
    #[test_case(&["0000-000a-0000000000000001-00", "0000-000a-0000000000000002-00"]
        => vec!["0000-000a-0000000000000001-00", "0000-000a-0000000000000002-00"] ; "different id same end kept")]
    fn dedupe(names: &[&str]) -> Vec<String> {
        let files: Vec<PackFileId> = names.iter().map(|s| s.parse().unwrap()).collect();
        // dedupe_pack_files returns HashMap order, so sort for a stable comparison
        let mut out: Vec<String> = dedupe_pack_files(&files)
            .iter()
            .map(|f| f.to_string())
            .collect();
        out.sort();
        out
    }
}
