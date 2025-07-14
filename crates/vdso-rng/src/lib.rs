#![no_std]
#![doc = include_str!("../README.md")]
extern crate alloc;

#[cfg(not(miri))]
mod auxv;
mod config;
mod pool;
mod utils;
#[cfg_attr(miri, path = "vdso_miri.rs")]
mod vdso;
use core::ffi::c_uint;
use linux_raw_sys::errno;
pub use pool::Pool;
use pool::Ptr;

/// Errors that may occur during vdso getrandom operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The shared state block pool has been poisoned.
    /// This should not happen with safe usage.
    PoolPoisoned,
    /// The operation is not supported on this platform.
    /// We failed to find the vdso symbol.
    NotSupported,
    /// Allocation failure occurred while trying to acquire a new random state.
    AllocationFailure,
    /// Normal errno as if it is returned from a system call.
    Errno(i32),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotSupported => write!(f, "Operation not supported on this platform"),
            Error::AllocationFailure => write!(f, "Failed to allocate memory"),
            Error::Errno(e) => write!(f, "System call failed with error code: {e}"),
            Error::PoolPoisoned => write!(f, "Memory pool has been poisoned"),
        }
    }
}

impl core::error::Error for Error {}

/// A local state for `vDSO`-based `getrandom` operations.
///
/// This state is rented from a shared [`Pool`] and used to fill buffers with random bytes.
///
/// ```rust
/// use vdso_rng::{Pool, LocalState};
///
/// let pool = Pool::new().expect("Failed to create shared pool");
/// let mut local_state = LocalState::new(&pool).expect("Failed to create local state");
///
/// let mut buf = [0u8; 64];
/// local_state.fill(&mut buf, 0).expect("Failed to fill buffer");
///
/// assert!(buf.iter().any(|&x| x != 0), "Buffer should not be empty");
/// ```
///
/// Typically, [`LocalState`] is intended to be used in a thread-local fashion. A thread rents
/// a state once and reuses it for multiple `getrandom` calls. Since acquiring a new state incurs
/// synchronization overhead, reusing the state within the same thread is strongly recommended.
///
/// On drop, the state is returned to the pool for reuse.
///
/// ## Safety
/// - **Not async-signal-safe**: [`LocalState`] is not safe to use within signal handlers. Reentrant usage
///   (e.g., invoking [`LocalState::fill`] from a signal handler that interrupts a [`LocalState::fill`]) can lead to secret leakage or corruption.
/// - **Reentrancy detection**: While Rust's borrowing rules typically prevent such misuse,
///   it may occur in the presence of undefined behavior. Under debug builds, reentrancy is explicitly checked
///   and will panic if detected.
pub struct LocalState<'a> {
    state: Ptr,
    pool: &'a Pool,
    #[cfg(debug_assertions)]
    inflight: bool,
}

impl<'a> LocalState<'a> {
    /// Create a new local state from the given pool. The pool must outlive the local state.
    pub fn new(pool: &'a Pool) -> Result<Self, Error> {
        let state = pool.get()?;
        Ok(Self {
            state,
            pool,
            #[cfg(debug_assertions)]
            inflight: false,
        })
    }
    /// Fill the provided buffer with random bytes. This method may not fill the entire buffer
    /// due to interrupts or low entropy conditions.
    pub fn try_fill(&mut self, buf: &mut [u8], flag: c_uint) -> Result<usize, Error> {
        let function = self.pool.config.function;
        let state = self.state.0.as_ptr();
        let state_length = self.pool.config.params.size_of_opaque_states as usize;
        let buffer_len = buf.len();
        #[cfg(debug_assertions)]
        {
            debug_assert!(
                !self.inflight,
                "LocalState is already in use, reentrancy detected"
            );
            self.inflight = true;
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
        unsafe {
            let result = function(
                buf.as_mut_ptr() as *mut _,
                buffer_len,
                flag,
                state,
                state_length,
            );
            #[cfg(debug_assertions)]
            {
                // Make sure all random bytes are written before moving on
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                self.inflight = false;
            }
            if result < 0 {
                return Err(Error::Errno(-result));
            }
            Ok(result as usize)
        }
    }

    /// Fill the provided buffer with random bytes. This method will block until the buffer is filled.
    /// It is implemented as a loop wrapping around [`LocalState::try_fill`].
    pub fn fill(&mut self, mut buf: &mut [u8], flag: c_uint) -> Result<(), Error> {
        while !buf.is_empty() {
            match self.try_fill(buf, flag) {
                Ok(filled) => {
                    buf = &mut buf[filled..];
                    continue;
                }
                Err(Error::Errno(e)) if e == errno::EAGAIN as i32 || e == errno::EINTR as i32 => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}

impl<'a> Drop for LocalState<'a> {
    fn drop(&mut self) {
        let state = self.state;
        self.pool.recycle(state);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::cell::RefCell;

    use alloc::vec::Vec;

    use super::*;
    #[test]
    fn get_local_state() {
        let pool = Pool::new().expect("Failed to create shared pool");
        _ = LocalState::new(&pool).expect("Failed to create local state");
    }

    #[test]
    fn fill_local_state() {
        let pool = Pool::new().expect("Failed to create shared pool");
        let mut local_state = LocalState::new(&pool).unwrap();
        let mut buf = [0u8; 64];
        let res = local_state.fill(&mut buf, 0);
        assert!(res.is_ok(), "Failed to fill local state: {:?}", res);
        assert!(buf.iter().any(|&x| x != 0), "Buffer should not be empty");
    }

    #[test]
    fn multi_local_state() {
        let pool = Pool::new().expect("Failed to create shared pool");
        let mut states = Vec::new();
        for _ in 0..128 {
            let local_state = LocalState::new(&pool).unwrap();
            states.push(local_state);
        }
        for state in states.iter_mut() {
            let mut buf = [0u8; 64];
            let res = state.fill(&mut buf, 0);
            assert!(res.is_ok(), "Failed to fill local state: {:?}", res);
            assert!(buf.iter().any(|&x| x != 0), "Buffer should not be empty");
        }
    }

    #[test]
    fn parallel_local_state() {
        let pool = Pool::new().expect("Failed to create shared pool");
        std::thread::scope(|scope| {
            let pool = &pool;
            for _ in 0..16 {
                scope.spawn(|| {
                    for _ in 0..16 {
                        let mut local_state = LocalState::new(pool).unwrap();
                        let mut buf = [0u8; 64];
                        let res = local_state.fill(&mut buf, 0);
                        assert!(res.is_ok(), "Failed to fill local state: {:?}", res);
                        assert!(buf.iter().any(|&x| x != 0), "Buffer should not be empty");
                    }
                });
            }
        });
    }

    #[test]
    fn global_state_test() {
        fn global_pool() -> &'static Pool {
            static GLOBAL_STATE: std::sync::LazyLock<Pool> =
                std::sync::LazyLock::new(|| Pool::new().expect("Failed to create global pool"));
            &GLOBAL_STATE
        }
        fn fill(buf: &mut [u8], flag: c_uint) -> Result<(), Error> {
            std::thread_local! {
                static LOCAL_STATE: RefCell<LocalState<'static>> = RefCell::new(LocalState::new(global_pool()).expect("Failed to create local state"));
            }
            LOCAL_STATE.with(|local_state| {
                let mut state = local_state.borrow_mut();
                state.fill(buf, flag)
            })
        }

        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    for _ in 0..16 {
                        let mut buf = [0u8; 64];
                        let res = fill(&mut buf, 0);
                        assert!(res.is_ok(), "Failed to fill global state: {:?}", res);
                        assert!(buf.iter().any(|&x| x != 0), "Buffer should not be empty");
                    }
                });
            }
        });
    }
}
