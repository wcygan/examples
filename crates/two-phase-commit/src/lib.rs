//! Small, in-memory two-phase commit state machines for teaching.
//! This crate deliberately makes no durability or high-availability claim.

mod coordinator;
mod participant;
mod state;

pub use coordinator::Coordinator;
pub use participant::Participant;
pub use state::{CoordinatorState, Decision, ParticipantState, ProtocolError, Vote};
