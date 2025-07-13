use core::{
    ptr::NonNull,
    sync::atomic::{AtomicPtr, Ordering},
};

use crate::{LockResult, bomb::HeavyWeightBomb, futex, rawlock::RawLock};

const SPIN_LIMIT: usize = 100;
const WAITING: u32 = 0;
const DONE: u32 = 1;
const HEAD: u32 = 2;
const SLEEPING: u32 = 3;
pub(crate) const POISONED: u32 = 4;

pub struct Node {
    futex: futex::Futex,
    next: AtomicPtr<Self>,
    closure: unsafe fn(NonNull<Self>),
}

impl Node {
    /// Creates a new `Node` with an initial state of `WAITING`.
    /// The `next` pointer is initialized to `null`.
    pub const fn new(closure: unsafe fn(NonNull<Self>)) -> Self {
        Self {
            futex: futex::Futex::new(WAITING),
            next: AtomicPtr::new(core::ptr::null_mut()),
            closure,
        }
    }

    /// Go to sleep until the futex is woken up with a message.
    pub fn wait(&self) -> u32 {
        match self
            .futex
            .compare_exchange(WAITING, SLEEPING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                self.futex.wait(SLEEPING);
                self.futex.load(Ordering::Acquire)
            }
            Err(value) => value,
        }
    }

    /// Wakes up the futex with a message.
    fn wake(&self, message: u32) {
        self.futex.notify(message, SLEEPING);
    }

    /// Wake up the futex with `DONE` message.
    pub fn wake_as_done(&self) {
        self.wake(DONE);
    }
    /// Wake up the futex with `HEAD` message.
    pub fn wake_as_head(&self) {
        self.wake(HEAD);
    }

    /// Wake up the futex with `POISONED` message.
    pub fn wake_as_poisoned(&self) {
        self.wake(POISONED);
    }

    /// Get the successor node.
    pub fn load_next(&self, ordering: Ordering) -> Option<NonNull<Self>> {
        let ptr = self.next.load(ordering);
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { NonNull::new_unchecked(ptr) })
        }
    }

    #[cfg(all(feature = "nightly", not(miri)))]
    pub unsafe fn prefetch_next(&self, ordering: Ordering) {
        let ptr = self.next.load(ordering);
        unsafe { core::intrinsics::prefetch_write_data(ptr, 3) };
    }

    /// Store the next node in the linked list.
    pub fn store_next(&self, next: NonNull<Self>) {
        self.next.store(next.as_ptr(), Ordering::Release);
    }

    /// Attach the node to a raw lock.
    pub fn attach(this: NonNull<Self>, raw: &RawLock) -> LockResult<()> {
        let mut bomb = HeavyWeightBomb::new(raw, this);
        match raw.swap_tail(this) {
            Some(prev) => unsafe {
                prev.as_ref().store_next(this);
                let mut status;
                'waiting: {
                    for _ in 0..SPIN_LIMIT {
                        status = this.as_ref().futex.load(Ordering::Acquire);
                        if status != WAITING {
                            break 'waiting;
                        }
                    }
                    status = this.as_ref().wait();
                }
                if status == DONE {
                    bomb.diffuse();
                    return Ok(());
                }
                if status == POISONED {
                    // defuse the bomb because we are not the head node.
                    bomb.diffuse();
                    return Err(crate::LockPoisoned);
                }
                debug_assert_eq!(status, HEAD);
            },
            None => {
                // we are going to be the head node.
                // If exiting early, we should trigger the bomb to propagate the poison.
                // This is needed because of the following scenario:
                // 1. Thread A graps the lock on fast path.
                // 2. Thread B tries to grap the lock and enters the slow path, which
                //    ends up spinning right here.
                // 3. Thread C enters the queue and waiting for the lock.
                // 4. Thread A panics, poisoning the lock.
                // 5. Thread B wakes up only to find that the lock is poisoned.
                // 6. Thread B needs to notify Thread C that the lock is poisoned.
                // 7. Thread C needs to wake up and handle the poison.
                raw.acquire()?;
            }
        }
        let mut cursor = this;
        loop {
            #[cfg(all(feature = "nightly", not(miri)))]
            unsafe {
                cursor.as_ref().prefetch_next(Ordering::Relaxed);
            }
            unsafe {
                (cursor.as_ref().closure)(cursor);
            }
            match unsafe { cursor.as_ref().load_next(Ordering::Acquire) } {
                Some(next) => {
                    unsafe { cursor.as_ref().wake_as_done() };
                    cursor = next;
                    bomb.reset(cursor);
                }
                None => break,
            }
        }

        if raw.try_close(cursor) {
            unsafe {
                cursor.as_ref().wake_as_done();
            }
            raw.release();
            bomb.diffuse();
            return Ok(());
        }

        loop {
            match unsafe { cursor.as_ref().load_next(Ordering::Acquire) } {
                Some(next) => unsafe {
                    next.as_ref().wake_as_head();
                    cursor.as_ref().wake_as_done();
                    bomb.diffuse();
                    return Ok(());
                },
                None => {
                    debug_assert!(raw.has_tail(Ordering::SeqCst));
                    continue;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::*;
    extern crate std;

    #[test]
    fn test_node_wait() {
        let node = Node::new(|_| {});
        std::thread::scope(|s| {
            {
                let node = &node;
                s.spawn(move || {
                    let result = node.wait();
                    assert_eq!(result, HEAD);
                });
            }
            node.wake(HEAD);
        })
    }

    #[test]
    fn test_node_next() {
        let node = Node::new(|_| {});
        std::thread::scope(|s| {
            {
                let node = &node;
                s.spawn(move || {
                    let local_node = Node::new(|_| {});
                    node.store_next(NonNull::from(&local_node));
                    assert_eq!(local_node.wait(), DONE);
                });
            }
            loop {
                match node.load_next(Ordering::Acquire) {
                    Some(next) => {
                        unsafe { next.as_ref().wake(DONE) };
                        break;
                    }
                    None => core::hint::spin_loop(),
                }
            }
        })
    }

    #[test]
    fn test_node_attach() {
        const NUM_THREADS: usize = 100;
        let counter = AssumeSync(Cell::new(0));
        struct AssumeSync<T>(T);
        unsafe impl<T> Sync for AssumeSync<T> {}
        let lock = RawLock::new();
        std::thread::scope(|s| {
            for _ in 0..NUM_THREADS {
                let counter = &counter;
                let lock = &lock;
                s.spawn(move || {
                    #[repr(C)]
                    struct CombinedNode<'a> {
                        node: Node,
                        counter: &'a AssumeSync<Cell<usize>>,
                    }
                    let combined_node = CombinedNode {
                        node: Node::new(|this| {
                            let container = this.cast::<CombinedNode>();
                            unsafe {
                                container
                                    .as_ref()
                                    .counter
                                    .0
                                    .set(container.as_ref().counter.0.get() + 1);
                            }
                        }),
                        counter,
                    };
                    Node::attach(NonNull::from(&combined_node).cast(), lock).unwrap();
                });
            }
        });
        assert_eq!(counter.0.get(), NUM_THREADS);
    }
}
