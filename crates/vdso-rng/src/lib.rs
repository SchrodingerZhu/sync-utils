extern crate alloc;

mod auxv;
mod pool;
mod vdso;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    PoolPoisoned,
    NotSupported,
    AllocationFailure,
    Reentrancy,
    Errno(i32),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotSupported => write!(f, "Operation not supported on this platform"),
            Error::AllocationFailure => write!(f, "Failed to allocate memory"),
            Error::Reentrancy => write!(f, "Reentrant call detected"),
            Error::Errno(e) => write!(f, "System call failed with error code: {e}"),
            Error::PoolPoisoned => write!(f, "Memory pool has been poisoned"),
        }
    }
}

impl core::error::Error for Error {}

use core::ffi::c_uint;

pub use pool::SharedPool;

use crate::pool::State;

pub struct LocalState<'a> {
    state: State,
    pool: &'a SharedPool,
}

impl<'a> LocalState<'a> {
    pub fn new(pool: &'a SharedPool) -> Result<Self, Error> {
        let state = pool
            .0
            .run(|x| x.pin_mut().get())
            .map_err(|_| Error::PoolPoisoned)??;
        Ok(Self { state, pool })
    }
    pub fn fill(&mut self, buf: &mut [u8], flag: c_uint) -> Result<usize, Error> {
        self.state.fill(buf, flag)
    }
}

impl<'a> Drop for LocalState<'a> {
    fn drop(&mut self) {
        let state = self.state;
        self.pool
            .0
            .run(move |x| x.pin_mut().recycle(state))
            .expect("Failed to recycle local state");
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{LazyCell, RefCell};

    use super::*;
    #[test]
    fn get_local_state() {
        if cfg!(miri) {
            return;
        }
        let pool = SharedPool::new().expect("Failed to create shared pool");
        _ = LocalState::new(&pool).expect("Failed to create local state");
    }

    #[test]
    fn fill_local_state() {
        if cfg!(miri) {
            return;
        }
        let pool = SharedPool::new().expect("Failed to create shared pool");
        let mut local_state = LocalState::new(&pool).unwrap();
        let mut buf = [0u8; 64];
        let res = local_state.fill(&mut buf, 0);
        assert!(res.is_ok(), "Failed to fill local state: {:?}", res);
        assert!(buf.iter().any(|&x| x != 0), "Buffer should not be empty");
    }

    #[test]
    fn multi_local_state() {
        if cfg!(miri) {
            return;
        }
        let pool = SharedPool::new().expect("Failed to create shared pool");
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
        if cfg!(miri) {
            return;
        }
        let pool = SharedPool::new().expect("Failed to create shared pool");
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
        if cfg!(miri) {
            return;
        }
        fn global_pool() -> &'static SharedPool {
            static GLOBAL_STATE: std::sync::LazyLock<SharedPool> = std::sync::LazyLock::new(|| {
                SharedPool::new().expect("Failed to create global pool")
            });
            &GLOBAL_STATE
        }
        fn fill(buf: &mut [u8], flag: c_uint) -> Result<usize, Error> {
            thread_local! {
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
            let mut handles = Vec::new();
            for _ in 0..16 {
                handles.push(scope.spawn(|| {
                    for _ in 0..16 {
                        let mut buf = [0u8; 64];
                        let res = fill(&mut buf, 0);
                        assert!(res.is_ok(), "Failed to fill global state: {:?}", res);
                        assert!(buf.iter().any(|&x| x != 0), "Buffer should not be empty");
                    }
                }));
            }
        });
    }
}
