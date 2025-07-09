#![no_std]

use core::{
    cell::{Cell, UnsafeCell},
    mem::MaybeUninit,
    ptr::NonNull,
};
mod raw;

pub struct LambdaLock<T> {
    raw: raw::LambdaLock,
    data: UnsafeCell<T>,
}

unsafe impl<T> Sync for LambdaLock<T> {}

impl<T> LambdaLock<T> {
    pub const fn new(data: T) -> Self {
        LambdaLock {
            raw: raw::LambdaLock::new(),
            data: UnsafeCell::new(data),
        }
    }

    #[inline(never)]
    fn run_slow<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R + Send + 'static,
        R: Send + 'static,
    {
        #[repr(C)]
        struct Node<F, T, R> {
            raw: raw::Node,
            closure: MaybeUninit<F>,
            data: NonNull<T>,
            result: Cell<MaybeUninit<R>>,
        }
        unsafe fn operate<F, T, R>(node: NonNull<raw::Node>)
        where
            F: FnOnce(&mut T) -> R,
            R: Send + 'static,
        {
            let casted = node.cast::<Node<F, T, R>>();
            let closure = unsafe { casted.as_ref().closure.assume_init_read() };
            let result = closure(unsafe { &mut *casted.as_ref().data.as_ptr() });
            unsafe {
                casted.as_ref().result.set(MaybeUninit::new(result));
            }
        }
        let node = Node {
            raw: raw::Node::new(operate::<F, T, R>),
            closure: MaybeUninit::new(f),
            data: unsafe { NonNull::new_unchecked(self.data.get()) },
            result: Cell::new(MaybeUninit::uninit()),
        };
        if node.raw.attach(&self.raw) {
            Some(unsafe { node.result.into_inner().assume_init() })
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn run<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R + Send + 'static,
        R: Send + 'static,
    {
        struct ReleaseGuard<'a> {
            raw: &'a raw::LambdaLock,
        }
        impl<'a> Drop for ReleaseGuard<'a> {
            fn drop(&mut self) {
                self.raw.release();
            }
        }
        if self.raw.try_lock()? {
            let _guard = ReleaseGuard { raw: &self.raw };
            let res = f(unsafe { &mut *self.data.get() });
            Some(res)
        } else {
            self.run_slow(f)
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;
    #[test]
    fn test_lambda_lock_add() {
        use super::LambdaLock;
        for _ in 0..100 {
            let lock = LambdaLock::new(0usize);
            // 100 threads increment the value by 1 in 100 iterations
            std::thread::scope(|s| {
                for _ in 0..100 {
                    s.spawn(|| {
                        for _ in 0..100 {
                            lock.run(|data| {
                                *data += 1;
                            });
                        }
                    });
                }
            });
            lock.run(|data| {
                assert_eq!(*data, 10000);
            });
        }
    }
    #[test]
    fn test_string_concatenation() {
        use super::LambdaLock;
        for _ in 0..100 {
            let lock = LambdaLock::new(alloc::string::String::new());
            std::thread::scope(|s| {
                for _ in 0..100 {
                    s.spawn(|| {
                        lock.run(|data| {
                            data.push_str("A");
                        });
                    });
                }
            });
            lock.run(|data| {
                assert_eq!(data.len(), 100,);
            });
        }
    }

    #[test]
    #[should_panic]
    fn test_panic_001() {
        use super::LambdaLock;
        let lock = &LambdaLock::new(0);
        std::thread::scope(|s| {
            for i in 0..100 {
                s.spawn(move || {
                    lock.run(move |data| {
                        *data += i;
                        if i == 50 {
                            panic!("Panic at {}", i);
                        }
                    })
                    .unwrap();
                });
            }
        });
    }
}
