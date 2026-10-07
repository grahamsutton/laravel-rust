//! The command bus: dispatching jobs, chains and batches.

pub mod batch;
pub mod chain;
pub mod dispatcher;
pub mod pending_dispatch;
pub mod repository;
pub mod unique;

pub use batch::{Batch, BatchItem, PendingBatch, UpdatedBatchJobCounts};
pub use chain::PendingChain;
pub use dispatcher::{Dispatcher, QueueingDispatcher};
pub use pending_dispatch::{Dispatchable, PendingDispatch, dispatch, dispatch_sync};
pub use repository::{BatchRecord, BatchRepository, InMemoryBatchRepository};
pub use unique::UniqueLock;

use crate::envelope::Envelope;
use crate::job::ShouldQueue;

/// Turn a list of boxed jobs into envelopes, flattening nested chains.
pub(crate) fn prepare_jobs(jobs: Vec<Box<dyn ShouldQueue>>) -> Vec<Envelope> {
    let mut envelopes = Vec::with_capacity(jobs.len());
    for job in jobs {
        if job.is::<PendingChain>() {
            let chain = job
                .into_any_box()
                .downcast::<PendingChain>()
                .expect("the job is a pending chain");
            envelopes.extend(chain.jobs);
        } else {
            envelopes.push(Envelope::from_box(job));
        }
    }
    envelopes
}
