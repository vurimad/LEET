use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Condvar, Mutex};

/// A counting semaphore that spins briefly before parking the thread.
///
/// Mirrors `DispatcherSemaphore` from the C++ original: avoids kernel calls
/// on the hot path by spinning first, then falling back to a condvar.
pub(crate) struct Semaphore {
    count: AtomicI32,
    mutex: Mutex<()>,
    condvar: Condvar,
}

/// Number of spin iterations before falling back to condvar sleep.
#[cfg(not(target_os = "linux"))]
const SPIN_COUNT: u32 = 10 * 1024;
#[cfg(target_os = "linux")]
const SPIN_COUNT: u32 = 1024; // sched_yield is expensive on Linux

impl Semaphore {
    pub(crate) fn new(initial: i32) -> Self {
        Self {
            count: AtomicI32::new(initial),
            mutex: Mutex::new(()),
            condvar: Condvar::new(),
        }
    }

    /// Decrement the semaphore, blocking if the count is zero.
    pub(crate) fn acquire(&self) {
        // Fast path: spin
        for _ in 0..SPIN_COUNT {
            let c = self.count.load(Ordering::Relaxed);
            if c > 0 {
                if self
                    .count
                    .compare_exchange_weak(c, c - 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
            }
            core::hint::spin_loop();
        }

        // Slow path: park on condvar
        let mut guard = self.mutex.lock().unwrap();
        loop {
            let c = self.count.load(Ordering::Acquire);
            if c > 0
                && self
                    .count
                    .compare_exchange(c, c - 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                return;
            }
            guard = self.condvar.wait(guard).unwrap();
        }
    }

    /// Try to decrement without blocking. Returns `true` on success.
    #[allow(dead_code)]
    pub(crate) fn try_acquire(&self) -> bool {
        let mut c = self.count.load(Ordering::Relaxed);
        while c > 0 {
            match self
                .count
                .compare_exchange_weak(c, c - 1, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return true,
                Err(new_c) => c = new_c,
            }
        }
        false
    }

    /// Increment the semaphore by `count`, waking up to `count` waiters.
    pub(crate) fn release(&self, count: i32) {
        self.count.fetch_add(count, Ordering::Release);
        if count == 1 {
            self.condvar.notify_one();
        } else {
            // Wake enough threads; extras will go back to sleep immediately.
            for _ in 0..count {
                self.condvar.notify_one();
            }
        }
    }
}
