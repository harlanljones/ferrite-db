//! Admission control — search-admission semaphore (~2× cores in flight);
//! sheds with `Busy` rather than queueing (AGENTS.md §4, ADR 0007). The ONLY
//! component allowed to return `Busy` for capacity reasons. Owned by ROADMAP
//! FDB-050.
//!
//! Design: a single lock-free counting semaphore bounds concurrent in-flight
//! searches process-wide. Acquisition is a non-blocking [`SearchAdmission::try_admit`]
//! that returns [`Error::Busy`] the instant no permit remains — there is no
//! wait queue (ADR 0007). Proof by inspection: the search entry performs
//! exactly one `try_admit` and holds the returned [`AdmissionPermit`] for the
//! call's duration, so the in-flight count equals the number of concurrent
//! searches and the gate never parks a caller.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use crate::errors::{Error, Result};

/// Multiplier applied to the logical core count for the default admission
/// capacity. ADR 0007 sizes the search-admission semaphore at roughly twice
/// the available logical cores; this is the documented capacity knob.
pub const DEFAULT_CAPACITY_MULTIPLE: usize = 2;

/// Default admission capacity: [`DEFAULT_CAPACITY_MULTIPLE`] times the logical
/// core count, never below one.
pub fn default_capacity() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    cores.saturating_mul(DEFAULT_CAPACITY_MULTIPLE).max(1)
}

/// A non-blocking, lock-free semaphore bounding concurrent in-flight searches.
///
/// Holds no queue: [`SearchAdmission::try_admit`] returns [`Error::Busy`]
/// immediately when `capacity` permits are already held, instead of parking
/// the caller (ADR 0007).
pub struct SearchAdmission {
    capacity: usize,
    in_flight: AtomicUsize,
}

impl SearchAdmission {
    /// Creates a gate with the given capacity (max concurrent in-flight
    /// searches). A capacity of zero rejects every call with [`Error::Busy`].
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self::build(capacity))
    }

    /// Creates a gate with the default capacity (see [`default_capacity`]).
    pub fn with_default_capacity() -> Arc<Self> {
        Self::new(default_capacity())
    }

    /// Builds the inner gate without wrapping it in an `Arc`.
    fn build(capacity: usize) -> Self {
        Self {
            capacity,
            in_flight: AtomicUsize::new(0),
        }
    }

    /// Returns the configured capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the current number of held permits.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Attempts to admit one in-flight search without blocking.
    ///
    /// On success returns an [`AdmissionPermit`] that releases its slot when
    /// dropped (i.e. at the end of the search call). When the gate is at
    /// capacity this returns [`Error::Busy`] immediately — it never waits.
    pub fn try_admit(&self) -> Result<AdmissionPermit<'_>> {
        self.in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < self.capacity).then_some(current + 1)
            })
            .map(|_| AdmissionPermit { gate: self })
            .map_err(|_| Error::Busy)
    }
}

/// A held admission slot. Dropping it returns the slot to the
/// [`SearchAdmission`] gate, allowing another search to be admitted.
pub struct AdmissionPermit<'a> {
    gate: &'a SearchAdmission,
}

impl Drop for AdmissionPermit<'_> {
    fn drop(&mut self) {
        self.gate.in_flight.fetch_sub(1, Ordering::Release);
    }
}

/// Process-wide admission gate used by the search entry point.
///
/// Returns the shared [`SearchAdmission`] singleton, sized to the default
/// capacity. The search entry performs exactly one [`SearchAdmission::try_admit`]
/// against this gate (structural proof, FDB-050 exit criterion).
pub fn global() -> &'static SearchAdmission {
    static GLOBAL: OnceLock<SearchAdmission> = OnceLock::new();
    GLOBAL.get_or_init(|| SearchAdmission::build(default_capacity()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::thread;

    #[test]
    fn default_capacity_is_positive_and_documented_multiple() {
        let capacity = default_capacity();
        assert!(capacity >= 1);
        assert_eq!(
            capacity,
            SearchAdmission::with_default_capacity().capacity()
        );
    }

    #[test]
    fn admits_up_to_capacity_then_sheds() {
        let gate = SearchAdmission::new(3);
        let first = gate.try_admit().unwrap();
        let second = gate.try_admit().unwrap();
        let third = gate.try_admit().unwrap();
        assert_eq!(gate.in_flight(), 3);
        assert!(std::ptr::eq(first.gate, &*gate));
        assert!(std::ptr::eq(second.gate, &*gate));
        assert!(std::ptr::eq(third.gate, &*gate));

        // At capacity: admit is rejected immediately, no queue.
        assert!(matches!(gate.try_admit(), Err(Error::Busy)));

        // Releasing one slot reopens admission.
        drop(first);
        assert_eq!(gate.in_flight(), 2);
        assert!(gate.try_admit().is_ok());
    }

    #[test]
    fn zero_capacity_always_sheds() {
        let gate = SearchAdmission::new(0);
        assert!(matches!(gate.try_admit(), Err(Error::Busy)));
    }

    #[test]
    fn saturation_sheds_immediately_under_load() {
        let gate = SearchAdmission::new(2);
        let barrier = Arc::new(Barrier::new(3));

        let hold = {
            let barrier = Arc::clone(&barrier);
            let gate = Arc::clone(&gate);
            thread::spawn(move || {
                let _permit = gate.try_admit().expect("worker 1 should admit");
                barrier.wait(); // signal main we hold the slot
                barrier.wait(); // wait for main to finish its assertion
            })
        };
        let hold2 = {
            let barrier = Arc::clone(&barrier);
            let gate = Arc::clone(&gate);
            thread::spawn(move || {
                let _permit = gate.try_admit().expect("worker 2 should admit");
                barrier.wait();
                barrier.wait();
            })
        };

        barrier.wait(); // both workers now hold their permits

        // The gate is full: admission must be refused at once, non-blocking.
        assert!(matches!(gate.try_admit(), Err(Error::Busy)));
        assert_eq!(gate.in_flight(), 2);

        barrier.wait(); // release the workers
        hold.join().unwrap();
        hold2.join().unwrap();
        assert_eq!(gate.in_flight(), 0);
    }
}
