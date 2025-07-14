#![no_std]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    PoolPoisoned,
    NotSupported,
    AllocationFailure,
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

pub struct LocalState<'a> {
    state: Ptr,
    pool: &'a Pool,
}

impl<'a> LocalState<'a> {
    pub fn new(pool: &'a Pool) -> Result<Self, Error> {
        let state = pool.get()?;
        Ok(Self { state, pool })
    }
    pub fn try_fill(&mut self, buf: &mut [u8], flag: c_uint) -> Result<usize, Error> {
        let function = self.pool.config.function;
        let state = self.state.0.as_ptr();
        let state_length = self.pool.config.params.size_of_opaque_states as usize;
        let buffer_len = buf.len();
        unsafe {
            let result = function(
                buf.as_mut_ptr() as *mut _,
                buffer_len,
                flag,
                state,
                state_length,
            );
            if result < 0 {
                return Err(Error::Errno(-result));
            }
            Ok(result as usize)
        }
    }
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
    use std::cell::{LazyCell, RefCell};

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
                static LOCAL_STATE: LazyCell<RefCell<LocalState<'static>>> = LazyCell::new(|| {
                    RefCell::new(LocalState::new(global_pool()).expect("Failed to create local state"))
                });
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
