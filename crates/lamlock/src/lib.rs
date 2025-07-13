#![no_std]
#![cfg_attr(all(feature = "nightly", not(miri)), allow(internal_features))]
#![cfg_attr(all(feature = "nightly", not(miri)), feature(core_intrinsics))]
#![doc = include_str!("../README.md")]
use core::{
    cell::{Cell, UnsafeCell},
    mem::MaybeUninit,
    ops::ControlFlow,
    ptr::NonNull,
    sync::atomic::Ordering,
};

use crate::node::Node;
mod bomb;
mod futex;
mod node;
mod rawlock;

/// Error type for when a lock is poisoned.
#[derive(Debug, Clone, Copy, Default)]
pub struct LockPoisoned;

/// Error type for when a lock is expected to be poisoned but is not.
#[derive(Debug, Clone, Copy, Default)]
pub struct LockNotPoisoned;

pub type LockResult<T> = Result<T, LockPoisoned>;

impl core::fmt::Display for LockPoisoned {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Lock is poisoned")
    }
}

impl core::error::Error for LockPoisoned {}

impl core::fmt::Display for LockNotPoisoned {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Lock is not poisoned")
    }
}

impl core::error::Error for LockNotPoisoned {}

/// The `Lock` struct is a thread-safe, poisonable lock that allows for safe concurrent access to data.
/// Create a new `Lock` with the [`Lock::new`] method.
/// To get access to the data, you can use the [`Lock::run`] method.
pub struct Lock<T> {
    raw: rawlock::RawLock,
    data: UnsafeCell<T>,
}

unsafe impl<T> Sync for Lock<T> {}

impl<T> Lock<T> {
    /// Create a new lock with the given data.
    pub const fn new(data: T) -> Self {
        Self {
            raw: rawlock::RawLock::new(),
            data: UnsafeCell::new(data),
        }
    }
    /// Wait until the lock is available, then poison it.
    /// Return error if the lock is already poisoned.
    pub fn poison(&self) -> Result<(), LockPoisoned> {
        self.raw.acquire()?;
        self.raw.poison();
        Ok(())
    }

    #[inline(never)]
    fn run_slowly<F, R>(&self, f: F) -> LockResult<R>
    where
        F: FnOnce(&mut T) -> R + Send,
        R: Send,
    {
        #[repr(C)]
        struct CombinedNode<'a, T, F, R> {
            node: Node,
            closure: MaybeUninit<F>,
            data: &'a UnsafeCell<T>,
            result: Cell<MaybeUninit<R>>,
        }
        unsafe fn execute<T, F, R>(this: NonNull<Node>)
        where
            F: FnOnce(&mut T) -> R,
        {
            let this = this.cast::<CombinedNode<T, F, R>>();
            let closure = unsafe { this.as_ref().closure.assume_init_read() };
            let data = unsafe { &mut *this.as_ref().data.get() };
            let result = (closure)(data);
            unsafe { this.as_ref().result.set(MaybeUninit::new(result)) };
        }
        let combined_node = CombinedNode {
            node: Node::new(execute::<T, F, R>),
            closure: MaybeUninit::new(f),
            data: &self.data,
            result: Cell::new(MaybeUninit::uninit()),
        };
        let this = NonNull::from(&combined_node).cast();
        Node::attach(this, &self.raw)?;
        Ok(unsafe { combined_node.result.into_inner().assume_init() })
    }

    /// Schedules a closure to run on the lock's data.
    /// The locking strategy splits into two paths:
    /// 1. If the lock is not poisoned and can be acquired immediately, it runs the closure directly.
    ///    On the fast path, the closure is not spilled into the node.
    /// 2. If the lock is poisoned or cannot be acquired immediately, it schedules the closure to run later.
    /// ```rust
    /// use lamlock::Lock;
    /// let lock = Lock::new(0);
    /// lock.run(|data| {
    ///   *data += 1;
    /// }).unwrap();
    /// ```
    #[inline(always)]
    pub fn run<F, R>(&self, f: F) -> LockResult<R>
    where
        F: FnOnce(&mut T) -> R + Send,
        R: Send,
    {
        if !self.raw.has_tail(Ordering::Relaxed) && self.raw.try_acquire()? {
            let bomb = bomb::LightWeightBomb::new(&self.raw);
            let result = f(unsafe { &mut *self.data.get() });
            self.raw.release();
            bomb.diffuse();
            return Ok(result);
        }
        self.run_slowly(f)
    }

    /// Try to inspect a poisoned lock. If the input closure returns [`ControlFlow::Continue`], the lock
    /// continues to be poisoned and the result is returned. If it returns [`ControlFlow::Break`], the lock
    /// is released to normal state.
    /// The function itself returns a [`Result<R, LockNotPoisoned>`], where `R` is the type of the result returned by the closure.
    /// If the lock is not poisoned when trying to acquire it as a poisoned lock, it returns [`LockNotPoisoned`].
    /// ```rust
    /// use lamlock::Lock;
    /// use std::ops::ControlFlow;
    /// let lock = Lock::new(0);
    /// lock.poison().unwrap();
    /// assert!(lock.run(|_| ()).is_err());
    /// lock.inspect_poison(|_| ControlFlow::Break(()));
    /// assert!(lock.run(|_| ()).is_ok());
    /// ```
    pub fn inspect_poison<F, R>(&self, f: F) -> Result<R, LockNotPoisoned>
    where
        F: FnOnce(&mut T) -> ControlFlow<R, R>,
    {
        self.raw.acquire_poison()?;
        match f(unsafe { &mut *self.data.get() }) {
            ControlFlow::Continue(result) => {
                self.raw.poison();
                Ok(result)
            }
            ControlFlow::Break(result) => {
                self.raw.release();
                Ok(result)
            }
        }
    }

    /// Unpoison the lock if it is poisoned.
    /// This is the same of calling [`Lock::inspect_poison`] with a closure that returns [`ControlFlow::Break`] with unit type.
    pub fn unpoison(&self) -> Result<(), LockNotPoisoned> {
        self.inspect_poison(|_| ControlFlow::Break(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate std;

    #[test]
    fn smoke_test() {
        let lock = Lock::new(0);
        lock.run(|data| {
            *data += 1;
        })
        .unwrap();
        assert_eq!(lock.run(|x| *x).unwrap(), 1);
    }

    #[test]
    fn multi_thread_test() {
        let cnt = 100;
        let lock = Lock::new(0);
        std::thread::scope(|scope| {
            for i in 0..cnt {
                let lock = &lock;
                scope.spawn(move || {
                    lock.run(|data| {
                        *data += cnt - i;
                    })
                    .unwrap();
                });
            }
        });

        assert_eq!(lock.run(|x| *x).unwrap(), cnt * (cnt + 1) / 2);
    }

    #[test]
    #[should_panic]
    fn mutli_thread_panic_chain_test() {
        let cnt = 100;
        let lock = Lock::new(0);
        std::thread::scope(|scope| {
            for i in 0..cnt {
                let lock = &lock;
                scope.spawn(move || {
                    lock.run(|data| {
                        *data += cnt - i;
                        if i == cnt / 2 {
                            panic!("panic chain");
                        }
                    })
                    .unwrap();
                });
            }
        });
    }

    #[test]
    fn multi_thread_inspect_poison() {
        let lock = Lock::new(std::string::String::new());
        std::thread::scope(|scope| {
            lock.poison().unwrap();
            lock.inspect_poison(|_| ControlFlow::Break(())).unwrap();
            lock.poison().unwrap();
            let mut handles = std::vec::Vec::new();
            for _ in 0..100 {
                let lock = &lock;
                handles.push(scope.spawn(move || {
                    if lock.run(|x| x.push('A')).is_err() {
                        lock.inspect_poison(|x| {
                            x.push('A');
                            ControlFlow::Break(())
                        })
                        .unwrap();
                    }
                }));
            }
            for handle in handles {
                handle.join().unwrap();
            }
            assert_eq!(lock.run(|x| x.len()).unwrap(), 100);
            assert_eq!(lock.run(|x| x.chars().all(|c| c == 'A')).unwrap(), true);
        });
    }
}
