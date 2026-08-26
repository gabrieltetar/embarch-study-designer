//! Capacity-bounded sequence storage, with one shape per target — design.md
//! §3 decisions 46 and 49.
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
//! `alloc` feature the backing store becomes a heap `Vec<T>` and the field
//! collapses to a pointer; without it — dev-bench's `no_std` firmware build —
//! the fixed-capacity form is retained, because that build has no allocator
//! and decision 15's reasoning is untouched there.
//!
//! Decision 46 introduced this for `Study.steps` alone. Decision 49 generalised
//! it, because `steps` was not the worst instance and not even the one that
//! crashed the *release* build: `StudyResult.steps` was a 64-slot array of
//! 20 KB `StepResult`s — **1.29 MB** — and `StepResult` itself was 20 KB
//! because `gatt_activity` inlined 32 × 536-byte records whether or not a step
//! captured any.
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
//! feature and fail under the other. [`Bounded::push`] is fallible in both
//! shapes — under `alloc` it enforces `N` explicitly rather than growing
//! without bound — so call sites are written once. Every read path (`iter`,
//! `len`, `get`, `last`, indexing, slicing) comes free via
//! `Deref<Target = [T]>` and needed no migration at all.
//!
//! # The capacity bound is preserved, deliberately
//!
//! Under `alloc` the limit is enforced in `push` *and* in `Deserialize`. That
//! second one matters: `Study` is deserialized from untrusted-ish input on the
//! host (`POST /study`, a saved study file), and `StudyResult` from
//! `events.json`. Dropping the bound along with the inline array would turn a
//! documented limit into an unbounded allocation driven by whatever the caller
//! sent — which is a worse bug than the one being fixed.

use core::ops::{Deref, DerefMut};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::limits::MAX_STEPS_PER_STUDY;
use crate::study::Step;

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
type Backing<T, const N: usize> = alloc::vec::Vec<T>;
#[cfg(not(feature = "alloc"))]
type Backing<T, const N: usize> = heapless::Vec<T, N>;

/// A sequence of at most `N` `T`s. See the module docs for why this is a
/// newtype and what differs per feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounded<T, const N: usize>(Backing<T, N>);

/// `Study.steps` (design.md §3 decision 46).
pub type StepList = Bounded<Step, MAX_STEPS_PER_STUDY>;

impl<T, const N: usize> Default for Bounded<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Bounded<T, N> {
    /// An empty sequence.
    pub fn new() -> Self {
        Self(Backing::new())
    }

    /// The maximum number of elements, in either shape.
    pub const CAPACITY: usize = N;

    /// Append an element, returning it back on overflow.
    ///
    /// Fallible in both shapes on purpose — see the module docs. The error
    /// type matches `heapless::Vec::push`'s (`Err(value)`) so the `no_std`
    /// call sites this replaced did not have to change at all.
    ///
    /// `clippy::result_large_err` is allowed because that match *is* the
    /// point: `T` may be large (a `Step` is ~600 bytes, a `StepResult` ~700),
    /// and boxing the error would need `alloc`, which the `no_std` build —
    /// the one that needs the signature to be unchanged — does not have.
    #[allow(clippy::result_large_err)]
    pub fn push(&mut self, value: T) -> Result<(), T> {
        #[cfg(feature = "alloc")]
        {
            if self.0.len() >= N {
                return Err(value);
            }
            self.0.push(value);
            Ok(())
        }
        #[cfg(not(feature = "alloc"))]
        {
            self.0.push(value)
        }
    }

    /// Whether another element would fit.
    pub fn is_full(&self) -> bool {
        self.0.len() >= N
    }
}

impl<T, const N: usize> Deref for Bounded<T, N> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T, const N: usize> DerefMut for Bounded<T, N> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a Bounded<T, N> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Collects up to `N` elements and **silently stops** there, matching
/// `heapless::Vec`'s own `FromIterator` behaviour rather than inventing a
/// different one per feature. Prefer [`Bounded::push`] where the overflow
/// needs to be observed.
impl<T, const N: usize> FromIterator<T> for Bounded<T, N> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut out = Self::new();
        for value in iter {
            if out.push(value).is_err() {
                break;
            }
        }
        out
    }
}

impl<T: Serialize, const N: usize> Serialize for Bounded<T, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Both backings serialize as a plain sequence, which is the whole
        // reason decisions 46/49 need no schema bump: nothing observable
        // crosses either hop differently.
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Bounded<T, N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let inner = Backing::<T, N>::deserialize(deserializer)?;
        #[cfg(feature = "alloc")]
        if inner.len() > N {
            // heapless enforces this for us; alloc does not, and dropping the
            // bound here would turn a documented limit into an unbounded
            // allocation driven by whatever the caller sent.
            return Err(serde::de::Error::custom("sequence exceeds its bound"));
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
        // Named accessors specifically — these are the `Deref` methods real
        // call sites use, so exercising them is the point (clippy would
        // rather see `!is_empty()`, which tests something else).
        assert_eq!(list.first().map(|s| s.timeout_ms), Some(1_000));
        assert_eq!(list.last().map(|s| s.timeout_ms), Some(1_000));
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

    /// Decision 49's own guard. `StudyResult` was **1,293,608 bytes** — a
    /// 64-slot inline array of 20 KB `StepResult`s — and per `embarch-core`'s
    /// own `build_runtime` comment it is the value that overflowed the stack
    /// of the real *release* Windows service, which decision 46's `Study` fix
    /// did not touch at all.
    #[cfg(feature = "alloc")]
    #[test]
    fn the_result_types_are_no_longer_dominated_by_inline_arrays() {
        use crate::result::{StepResult, StudyResult};

        let study_result = core::mem::size_of::<StudyResult>();
        let step_result = core::mem::size_of::<StepResult>();

        // Ceilings with real headroom, not pins. Both were more than an order
        // of magnitude above these before decision 49.
        assert!(
            step_result <= 4_096,
            "StepResult should no longer inline its GATT arrays, got {step_result}"
        );
        assert!(
            study_result <= 65_536,
            "StudyResult should no longer inline 64 StepResults, got {study_result}"
        );
    }

    /// The `no_std` build must keep the fixed-capacity shape — dev-bench has
    /// no allocator, and decision 15 is unchanged there. Asserted so "make it
    /// smaller" can never be applied to the one build that cannot afford an
    /// allocator.
    #[cfg(not(feature = "alloc"))]
    #[test]
    fn the_no_std_build_keeps_its_fixed_capacity_arrays() {
        assert!(
            core::mem::size_of::<crate::result::StepResult>() > 4_096,
            "the no_std shape must still inline its arrays"
        );
    }

    /// A `Bounded` and the `heapless::Vec` it replaces must be
    /// indistinguishable on the wire — the whole basis for decisions 46/49
    /// needing no schema bump. Asserted for a non-`Step` element type too,
    /// since decision 49 applied the newtype to three more fields with
    /// different element types.
    #[test]
    fn any_element_type_is_wire_compatible_with_the_heapless_shape() {
        let mut bounded: Bounded<u32, 8> = Bounded::new();
        let mut legacy: heapless::Vec<u32, 8> = heapless::Vec::new();
        for v in [1u32, 2, 3] {
            bounded.push(v).unwrap();
            legacy.push(v).unwrap();
        }

        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        assert_eq!(
            postcard::to_slice(&bounded, &mut a).unwrap(),
            postcard::to_slice(&legacy, &mut b).unwrap()
        );
    }
}
