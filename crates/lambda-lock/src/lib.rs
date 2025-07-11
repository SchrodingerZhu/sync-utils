#![no_std]

use core::{
    cell::{Cell, UnsafeCell},
    mem::MaybeUninit,
    ptr::NonNull,
    sync::atomic::Ordering,
};

use crate::node::Node;
mod bomb;
mod futex;
mod node;
mod rawlock;

#[derive(Debug, Clone, Copy, Default)]
pub struct LockPoisoned;

pub type LockResult<T> = Result<T, LockPoisoned>;

impl core::fmt::Display for LockPoisoned {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Lock is poisoned")
    }
}

impl core::error::Error for LockPoisoned {}

pub struct Lock<T> {
    raw: rawlock::RawLock,
    data: UnsafeCell<T>,
}

unsafe impl<T> Sync for Lock<T> {}

impl<T> Lock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            raw: rawlock::RawLock::new(),
            data: UnsafeCell::new(data),
        }
    }
    #[inline(never)]
    fn run_slowly<F, R>(&self, f: F) -> LockResult<R>
    where
        F: FnOnce(&mut T) -> R + Send,
        R: Send,
    {
        #[repr(C)]
        struct CombinedNode<'a, T, F, R> {
            node: UnsafeCell<Node>,
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
            node: UnsafeCell::new(Node::new(execute::<T, F, R>)),
            closure: MaybeUninit::new(f),
            data: &self.data,
            result: Cell::new(MaybeUninit::uninit()),
        };
        let this = NonNull::from(&combined_node).cast();
        Node::attach(this, &self.raw)?;
        Ok(unsafe { combined_node.result.into_inner().assume_init() })
    }

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
}
