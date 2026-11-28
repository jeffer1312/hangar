//! Estado durável, lotes e recibos do runtime.
#![deny(unsafe_code)]

mod queue;
mod receipt;

pub use hangar_codex::proto::RequestId;
pub use hangar_core::{clock::ClockSample, net::MAX_FRAME};
pub use queue::{
    Action, BATCH_SIZE_MESSAGE, BATCH_TOO_LARGE, BatchValidator, CAP, CODEX_LIFE, DeliveryObserver,
    Lease, Operation, QueueActor, State, Status, Store, acquire_lease, acquire_lease_within,
    discard, legacy_rows, oldest_waiting, projection_path, queue_text_ok, refusal, waiting_ids,
};
pub use receipt::{DispatchCursor, Occurrence, ReceiptIndex, ReceiptProof, image_receipt};
