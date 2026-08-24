//! Per-Table single-writer/many-reader coordination.
//!
//! Readers clone the immutable publication snapshot while holding the
//! read-lock, then traverse that snapshot without retaining any coordination
//! lock. Commits serialize through the writer lock and atomically publish the
//! replacement snapshot. Compute stays synchronous and runs on a library-owned
//! Rayon pool (ADR 0004).

use std::sync::{Arc, Mutex, RwLock};

use rayon::{ThreadPool, ThreadPoolBuilder};

use crate::errors::{Error, Result};

/// Coordinates publication and concurrent access for one Table.
#[derive(Debug)]
pub struct TableCoordinator<T> {
    writer: Mutex<()>,
    published: RwLock<Arc<T>>,
    pool: ThreadPool,
}

impl<T: Send + Sync> TableCoordinator<T> {
    /// Creates a coordinator with the initial publication snapshot.
    pub fn new(initial: T) -> Result<Self> {
        let pool = ThreadPoolBuilder::new()
            .build()
            .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?;
        Ok(Self {
            writer: Mutex::new(()),
            published: RwLock::new(Arc::new(initial)),
            pool,
        })
    }

    /// Clones the current immutable snapshot for a reader.
    ///
    /// The lock is held only while cloning the `Arc`; callers traverse the
    /// returned snapshot after this method returns.
    pub fn snapshot(&self) -> Result<Arc<T>> {
        self.install(|| {
            self.published
                .read()
                .map(|snapshot| Arc::clone(&snapshot))
                .map_err(|_| lock_error("publication snapshot read lock poisoned"))
        })
    }

    /// Serializes a Commit and atomically publishes its replacement snapshot.
    pub fn commit(&self, replacement: T) -> Result<()> {
        let _writer = self
            .writer
            .lock()
            .map_err(|_| lock_error("Table writer lock poisoned"))?;
        let mut published = self
            .published
            .write()
            .map_err(|_| lock_error("publication snapshot write lock poisoned"))?;
        *published = Arc::new(replacement);
        Ok(())
    }

    /// Number of workers in the library-owned Rayon pool.
    pub fn pool_threads(&self) -> usize {
        self.pool.current_num_threads()
    }

    /// Runs an internal synchronous operation on the library-owned pool.
    pub(crate) fn install<R>(&self, operation: impl FnOnce() -> R + Send) -> R
    where
        R: Send,
    {
        self.pool.install(operation)
    }
}

fn lock_error(detail: &str) -> Error {
    Error::Io(std::io::Error::other(detail))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    #[test]
    fn readers_hold_an_immutable_snapshot_after_commit() {
        let coordinator = TableCoordinator::new(vec![1_u64, 2]).unwrap();
        let before = coordinator.snapshot().unwrap();
        coordinator.commit(vec![3, 4, 5]).unwrap();
        let after = coordinator.snapshot().unwrap();

        assert_eq!(&*before, &[1, 2]);
        assert_eq!(&*after, &[3, 4, 5]);
    }

    #[test]
    fn commits_are_serialized_and_readers_see_only_complete_snapshots() {
        let coordinator = Arc::new(TableCoordinator::new(vec![0_u64; 8]).unwrap());
        let start = Arc::new(Barrier::new(5));

        let writers = (1..=4)
            .map(|value| {
                let coordinator = Arc::clone(&coordinator);
                let start = Arc::clone(&start);
                thread::spawn(move || {
                    start.wait();
                    for _ in 0..64 {
                        coordinator.commit(vec![value; 8]).unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();

        start.wait();
        for _ in 0..256 {
            let snapshot = coordinator.snapshot().unwrap();
            assert_eq!(snapshot.len(), 8);
            assert!(
                snapshot
                    .iter()
                    .all(|value| (1..=4).contains(value) || *value == 0)
            );
        }
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(coordinator.snapshot().unwrap().len(), 8);
    }

    #[test]
    fn pool_is_library_owned_and_runs_synchronous_work() {
        let coordinator = TableCoordinator::new(21_u32).unwrap();
        assert!(coordinator.pool_threads() >= 1);
        let result = coordinator.install(|| 20_u32 + 1);
        assert_eq!(result, 21);
    }
}
