use crate::{
    crypto::{Signature, SignatureVerificationError, VerifyingKey},
    ids::LogId,
    log_entry::RootInfo,
};

async fn admit() {}

fn verify_head(
    log_id: &LogId,
    head: &RootInfo,
    sig: &Signature,
    key: &VerifyingKey,
) -> Result<(), SignatureVerificationError> {
    let sign_bytes = head.sign_bytes(log_id);
    key.verify_signature(&sign_bytes, sig)
}
