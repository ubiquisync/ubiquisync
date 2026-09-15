use thiserror::Error;

use crate::{
    crypto::{EntryCipher, SegmentCipher, VerifyingKey},
    log::{
        ChainSeed, SegmentCipherError,
        segment::{SegmentDecodeError, SegmentReader, SegmentVerifyError},
    },
};

#[derive(Error, Debug)]
pub enum JoinSegmentsError {
    #[error("segment decode error {0}")]
    Decode(#[from] SegmentDecodeError),
    #[error("segment verify error {0}")]
    Verify(#[from] SegmentVerifyError),
    #[error("segment cipher error {0}")]
    Cipher(#[from] SegmentCipherError),
}

pub fn join_segments<'a>(
    verifying_key: &VerifyingKey,
    entry_cipher: &Option<EntryCipher>,
    segment_cipher: &Option<SegmentCipher>,
    seed: &ChainSeed,
    bodies: impl Iterator<Item = &'a [u8]>,
) -> Result<(), JoinSegmentsError> {
    //     let seed = ChainSeed::new(&log_id);
    let mut prev_chain = None;
    let mut chain_hash = None;
    let mut all_entries = vec![];
    let mut sig = None;
    for body in bodies {
        let reader = SegmentReader::start(body)?;
        let segment_header = reader.header();
        if let Some(ch) = chain_hash {
            if segment_header.prev_chain != ch {
                todo!("error")
            }
        } else {
            prev_chain = Some(segment_header.prev_chain);
        }
        let verified = reader.verify(verifying_key, entry_cipher, segment_cipher, seed)?;
        chain_hash = Some(verified.chain_hash);
        sig = Some(verified.signature);
        let entries = verified.to_plaintext(entry_cipher)?;
        all_entries.extend(entries);
    }

    todo!()
}

// pub fn join_segments<'a>(
//     &self,
//     peer_info: &PeerInfo,
//     container_id: &ContainerId,
//     bodies: impl Iterator<Item = &'a [u8]>,
// ) -> Result<SegmentData, JoinSegmentsError> {
//     let log_id = LogId {
//         peer_id: peer_info.peer,
//         container_id: *container_id,
//     };
//     let seed = ChainSeed::new(&log_id);
//     let mut prev_chain = None;
//     let mut chain_hash = None;
//     let mut all_entries = vec![];
//     let mut sig = None;
//     for body in bodies {
//         let reader = SegmentReader::start(body)?;
//         let segment_header = reader.header();
//         if let Some(ch) = chain_hash {
//             if segment_header.prev_chain != ch {
//                 todo!("error")
//             }
//         } else {
//             prev_chain = Some(segment_header.prev_chain);
//         }
//         let verified = reader.verify(&peer_info.commitment.sig_verify_key, &None, &None, &seed)?;
//         chain_hash = Some(verified.chain_hash);
//         sig = Some(verified.signature);
//         let entries = verified.to_plaintext(&None)?;
//         all_entries.extend(entries);
//     }

//     // TODO get rid of unwraps
//     let sig = sig.unwrap();
//     let prev_chain = prev_chain.unwrap();
//     let chain_hash = chain_hash.unwrap();

//     let new_body = encode_segment_plaintext(&sig, &prev_chain, &None, &all_entries)?;
//     Ok(SegmentData {
//         body: new_body,
//         prev_chain,
//         chain_hash,
//     })
// }
