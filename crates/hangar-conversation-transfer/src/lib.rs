//! Conversão integral do histórico Claude e registro durável da transferência.
#![forbid(unsafe_code)]

mod conversion;
mod error;
mod store;

pub use conversion::{
    ImportedContext, MAX_REQUEST_BYTES, convert_bytes, convert_snapshot, reconstruct_chain,
    serialized_batches,
};
pub use error::TransferError;
pub use store::{
    ConversationSource, ImportBoundary, IncompleteScan, Store, TransferPhase, TransferRecord,
    canonical_id, digest, map_permission, prepare_origin, verify_source,
};
