//! `Study.steps`' storage, with one shape per target — design.md §3 decision 46.
//!
//! # Why this type exists
//!
//! `Step`'s largest `Action` variant carries a `MAX_PAYLOAD_LEN` (512-byte)
//! payload, so a `heapless::Vec<Step, MAX_STEPS_PER_STUDY>` is a 64-slot
//! inline array of ~600-byte elements — roughly 38 KB of *value*, moved on the
//! stack every time a `Study` is passed around, regardless of how many steps
//! are actually populated. For a real two-step self-test study, that is 38 KB
//! to carry two steps. It is what genuinely crashed a debug `embarch-api`
//! serving a live `study_status` call over MCP.
//!
//! On a host with an allocator there is no reason to pay that. Behind the
//! `alloc` feature the backing store becomes a heap `Vec<Step>` and the field
//! collapses to a pointer; without it — dev-bench's `no_std` firmware build —
//! the fixed-capacity form is retained, because that build has no allocator
//! and decision 15's reasoning is untouched there.
//!
//! # This narrows decision 15, it does not overturn it
//!
//! Decision 15 says every sequence field is `heapless`, and this crate
//! "never requires a global allocator, with or without `std`." That remains
//! true of every build dev-bench makes and of the crate's default features.
//! What changes is that a host consumer may now *opt in* to an allocator for
//! this one field, which is the only field where the fixed-capacity form has
//! ever actually cost anything.
//!
//! # Why a newtype rather than a bare `cfg`-swapped alias
//!
//! `heapless::Vec::push` returns `Result<(), T>`; `alloc::vec::Vec::push`
//! returns `()`. A bare alias would make every call site compile under one
//! feature and fail under the other. [`StepList::push`] is fallible in both
//! shapes — under `alloc` it enforces [`MAX_STEPS_PER_STUDY`] explicitly
//! rather than growing without bound — so call sites are written once. Every
//! read path (`iter`, `len`, `get`, `last`, indexing, slicing) comes free via
//! `Deref<Target = [Step]>` and needed no migration at all.
//!
//! # The capacity bound is preserved, deliberately
//!
//! Under `alloc` the limit is enforced in `push` *and* in `Deserialize`. That
//! second one matters: `Study` is deserialized from untrusted-ish input on the
//! host (`POST /study`, a saved study file), and dropping the bound along with
//! the inline array would turn a documented limit into an unbounded allocation
//! driven by whatever the caller sent.

use core::ops::{Deref, DerefMut};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::limits::MAX_STEPS_PER_STUDY;
use crate::study::Step;

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
type Backing = alloc::vec::Vec<Step>;
#[cfg(not(feature = "alloc"))]
type Backing = heapless::Vec<Step, MAX_STEPS_PER_STUDY>;

/// The ordered steps of a [`Study`](crate::study::Study). See the module
/// docs for why this is a newtype and what differs per feature.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StepList(Backing);

impl StepList {
    /// An empty list.
    pub fn new() -> Self {
        Self(Backing::new())
    }

    /// The maximum number of steps a study may contain, in either shape.
    pub const CAPACITY: usize = MAX_STEPS_PER_STUDY;

    /// Append a step, returning it back on overflow.
    ///
    /// Fallible in both shapes on purpose — see the module docs. The error
    /// type matches `heapless::Vec::push`'s (`Err(value)`) so the `no_std`
    /// call sites this replaced did not have to change at all.
    /// `clippy::result_large_err` fires because `Step` is ~600 bytes and the
    /// `Err` variant hands it back. That is deliberate and is exactly
    /// `heapless::Vec::push`'s own signature — matching it is what let every
    /// `no_std` call site migrate to this type without changing a line.
    /// Boxing the error would need `alloc`, which the `no_std` build does not
    /// have, so the lint's suggested fix is unavailable in the shape that
    /// needs it most.
    #[allow(clippy::result_large_err)]
    pub fn push(&mut self, step: Step) -> Result<(), Step> {
        #[cfg(feature = "alloc")]
        {
            if self.0.len() >= MAX_STEPS_PER_STUDY {
                return Err(step);
            }
            self.0.push(step);
            Ok(())
        }
        #[cfg(not(feature = "alloc"))]
        {
            self.0.push(step)
        }
    }

    /// Whether another step would fit.
    pub fn is_full(&self) -> bool {
        self.0.len() >= MAX_STEPS_PER_STUDY
    }
}

impl Deref for StepList {
    type Target = [Step];
    fn deref(&self) -> &[Step] {
        &self.0
    }
}

impl DerefMut for StepList {
    fn deref_mut(&mut self) -> &mut [Step] {
        &mut self.0
    }
}

impl<'a> IntoIterator for &'a StepList {
    type Item = &'a Step;
    type IntoIter = core::slice::Iter<'a, Step>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Collects up to [`MAX_STEPS_PER_STUDY`] steps and **silently stops** there,
/// matching `heapless::Vec`'s own `FromIterator` behaviour rather than
/// inventing a different one per feature. Prefer [`StepList::push`] where the
/// overflow needs to be observed.
impl FromIterator<Step> for StepList {
    fn from_iter<I: IntoIterator<Item = Step>>(iter: I) -> Self {
        let mut out = Self::new();
        for step in iter {
            if out.push(step).is_err() {
                break;
            }
        }
        out
    }
}

impl Serialize for StepList {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Both backings serialize as a plain sequence, which is the whole
        // reason decision 46 needs no schema bump: nothing observable crosses
        // either hop differently.
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StepList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let inner = Backing::deserialize(deserializer)?;
        #[cfg(feature = "alloc")]
        if inner.len() > MAX_STEPS_PER_STUDY {
            // heapless enforces this for us; alloc does not, and dropping the
            // bound here would turn a documented limit into an unbounded
            // allocation driven by whatever the caller sent.
            return Err(serde::de::Error::custom("too many steps"));
        }
        Ok(Self(inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::{Action, BleRole};

    fn a_step() -> Step {
        Step {
            name: heapless::String::try_from("s").unwrap(),
            action: Action::BleConnect {
                role: BleRole::Central,
                target_address: None,
                target_name: None,
            },
            timeout_ms: 1_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        }
    }

    /// The cap survives the change of backing store. Under `heapless` it is
    /// structural; under `alloc` it is enforced by hand, and this is what
    /// says so out loud.
    #[test]
    fn push_refuses_to_exceed_the_capacity_in_either_shape() {
        let mut list = StepList::new();
        for _ in 0..MAX_STEPS_PER_STUDY {
            list.push(a_step()).expect("should fit");
        }
        assert_eq!(list.len(), MAX_STEPS_PER_STUDY);
        assert!(list.is_full());
        assert!(list.push(a_step()).is_err(), "capacity must still bind");
    }

    /// The read paths every migrated call site relies on, via `Deref`.
    #[test]
    fn read_paths_work_through_deref() {
        let mut list = StepList::new();
        list.push(a_step()).unwrap();
        list.push(a_step()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.iter().count(), 2);
        assert!(list.first().is_some());
        assert!(list.last().is_some());
        list[0].timeout_ms = 42;
        assert_eq!(list[0].timeout_ms, 42);
    }

    /// Decision 46 claims no schema bump because both backings serialize
    /// identically. Asserted rather than asserted-in-prose: the bytes must
    /// match a plain `heapless::Vec` of the same steps, which is what the
    /// pre-decision wire shape was.
    #[test]
    fn serializes_byte_identically_to_the_heapless_shape_it_replaced() {
        let mut list = StepList::new();
        let mut legacy: heapless::Vec<Step, MAX_STEPS_PER_STUDY> = heapless::Vec::new();
        for _ in 0..3 {
            list.push(a_step()).unwrap();
            legacy.push(a_step()).unwrap();
        }

        let mut a = [0u8; 4096];
        let mut b = [0u8; 4096];
        let from_list = postcard::to_slice(&list, &mut a).unwrap();
        let from_legacy = postcard::to_slice(&legacy, &mut b).unwrap();
        assert_eq!(from_list, from_legacy, "decision 46 must not change the wire");

        let round_tripped: StepList = postcard::from_bytes(from_legacy).unwrap();
        assert_eq!(round_tripped.len(), 3);
    }

    /// The bound has to hold on the way *in* too, not only on `push` — a
    /// `Study` is deserialized from `POST /study` and from saved study files.
    #[cfg(feature = "alloc")]
    #[test]
    fn deserialize_rejects_more_steps_than_the_capacity() {
        let overlong: alloc::vec::Vec<Step> =
            (0..MAX_STEPS_PER_STUDY + 1).map(|_| a_step()).collect();
        let mut buf = [0u8; 65536];
        let encoded = postcard::to_slice(&overlong, &mut buf).unwrap();
        assert!(
            postcard::from_bytes::<StepList>(encoded).is_err(),
            "an over-capacity sequence must be refused, not allocated"
        );
    }

    /// The regression guard design.md §3 decision 46 asks for by name: the
    /// next time someone grows `Step` or `MAX_STEPS_PER_STUDY`, this fails in
    /// CI rather than as a stack overflow in production.
    ///
    /// Deliberately a ceiling, not an equality — exact layout is not a
    /// contract, and pinning it would break on an unrelated field reorder.
    #[test]
    fn step_list_is_a_pointer_not_an_inline_array_under_alloc() {
        let inline = core::mem::size_of::<heapless::Vec<Step, MAX_STEPS_PER_STUDY>>();

        #[cfg(feature = "alloc")]
        {
            let actual = core::mem::size_of::<StepList>();
            assert!(
                actual <= 64,
                "StepList should be a handful of words under `alloc`, got {actual}"
            );
            assert!(
                actual * 100 < inline,
                "the whole point of decision 46: {actual} must be far below the \
                 inline array's {inline} bytes"
            );
        }

        #[cfg(not(feature = "alloc"))]
        {
            // Without an allocator the inline array is the only option, and
            // that is the accepted cost on dev-bench (decision 15).
            assert_eq!(core::mem::size_of::<StepList>(), inline);
        }
    }
}
