//! Testing helpers: the queue and bus fakes.
//!
//! `Queue::fake()` and `Bus::fake()` swap fakes into the container that
//! record jobs instead of running them, so you may assert on what your
//! code dispatched.

mod bus_fake;
mod queue_fake;

pub use bus_fake::BusFake;
pub use queue_fake::{PushedJob, QueueFake, RawPush};

use std::any::TypeId;

use crate::envelope::Envelope;
use crate::job::ShouldQueue;

/// A list of job types, written as a tuple: `(ShipOrder, RecordShipment)`.
///
/// Used by the chain assertions:
/// `Bus::assert_chained::<(ShipOrder, RecordShipment, UpdateInventory)>()`.
pub trait JobTypes {
    /// The registered names of the job types, in order.
    fn job_names() -> Vec<&'static str>;
}

macro_rules! job_types_for_tuples {
    ($(($($job:ident),+)),+ $(,)?) => {
        $(
            impl<$($job: ShouldQueue),+> JobTypes for ($($job,)+) {
                fn job_names() -> Vec<&'static str> {
                    vec![$($job::job_name()),+]
                }
            }
        )+
    };
}

job_types_for_tuples!(
    (A),
    (A, B),
    (A, B, C),
    (A, B, C, D),
    (A, B, C, D, E),
    (A, B, C, D, E, F),
    (A, B, C, D, E, F, G),
    (A, B, C, D, E, F, G, H),
    (A, B, C, D, E, F, G, H, I),
    (A, B, C, D, E, F, G, H, I, J),
);

impl JobTypes for () {
    fn job_names() -> Vec<&'static str> {
        Vec::new()
    }
}

/// Which job types a fake should (or should not) fake.
#[derive(Debug, Default)]
pub(crate) struct FakeFilter {
    only: Vec<TypeId>,
    except: Vec<TypeId>,
}

impl FakeFilter {
    pub(crate) fn only(&mut self, type_id: TypeId) {
        self.only.push(type_id);
    }

    pub(crate) fn except(&mut self, type_id: TypeId) {
        self.except.push(type_id);
    }

    /// Determine if the job should be faked (recorded) rather than run.
    pub(crate) fn should_fake(&self, job: &Envelope) -> bool {
        let type_id = job.job().as_any().type_id();
        if self.except.contains(&type_id) {
            return false;
        }
        self.only.is_empty() || self.only.contains(&type_id)
    }
}

/// The display name of the job type `T`, for assertion messages.
pub(crate) fn job_type_name<T: ShouldQueue>() -> &'static str {
    T::job_name()
}
