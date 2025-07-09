use core::{
    ptr::NonNull,
    sync::atomic::{AtomicPtr, AtomicU32, Ordering},
};

use rustix::thread::futex::Flags;

const SPIN_LIMIT: usize = 100;

#[repr(u32)]
#[derive(Eq, PartialEq, Clone, Copy)]
enum NodeStatus {
    Waiting,
    Done,
    Head,
    Sleeping,
    Poisoned,
}

#[repr(u32)]
#[derive(Eq, PartialEq, Clone, Copy)]
enum LockStatus {
    Unlocked,
    Locked,
    Poisoned,
}

pub(crate) struct LambdaLock {
    tail: AtomicPtr<Node>,
    status: AtomicU32,
}

pub(crate) struct Node {
    next: AtomicPtr<Node>,
    status: AtomicU32,
    task: unsafe fn(NonNull<Node>),
}

struct Bomb {
    lock: NonNull<LambdaLock>,
    current: NonNull<Node>,
}

impl Bomb {
    fn defuse(self) {
        core::mem::forget(self);
    }
    fn chain_reaction(lock: NonNull<LambdaLock>, mut current: NonNull<Node>) {
        unsafe {
            loop {
                let next = current.as_ref().next.load(Ordering::Acquire);
                match NonNull::new(next) {
                    Some(next) => {
                        current.as_ref().wake(NodeStatus::Poisoned);
                        current = next;
                    }
                    None => {
                        break;
                    }
                }
                if lock
                    .as_ref()
                    .tail
                    .compare_exchange(
                        current.as_ptr(),
                        core::ptr::null_mut(),
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    current.as_ref().wake(NodeStatus::Poisoned);
                    return;
                }

                while current.as_ref().next.load(Ordering::Relaxed).is_null() {
                    // Spin until the next node is set
                    core::hint::spin_loop();
                }
            }
        }
    }
}

impl Drop for Bomb {
    #[cold]
    fn drop(&mut self) {
        // force close the queue
        unsafe {
            self.lock
                .as_ref()
                .status
                .store(LockStatus::Poisoned as u32, Ordering::Release);
        }
        // Start the chain reaction
        Self::chain_reaction(self.lock, self.current);
    }
}

impl Node {
    pub(crate) const fn new(task: unsafe fn(NonNull<Node>)) -> Self {
        Node {
            next: AtomicPtr::new(core::ptr::null_mut()),
            status: AtomicU32::new(NodeStatus::Waiting as u32),
            task,
        }
    }

    #[cfg(miri)]
    fn wake(&self, message: NodeStatus) {
        self.status.store(message as u32, Ordering::Release);
    }

    #[cfg(not(miri))]
    fn wake(&self, message: NodeStatus) {
        if self.status.swap(message as u32, Ordering::AcqRel) == NodeStatus::Sleeping as u32 {
            _ = rustix::thread::futex::wake(&self.status, Flags::PRIVATE, 1)
        }
    }

    #[cfg(miri)]
    fn wait(&self) {
        while self.status.load(Ordering::Acquire) == NodeStatus::Waiting as u32 {
            core::hint::spin_loop();
        }
    }
    #[cfg(not(miri))]
    fn wait(&self) {
        #[cfg(miri)]
        {
            while self.status.load(Ordering::Acquire) == NodeStatus::Waiting as u32 {
                core::hint::spin_loop();
            }
        }
        let mut remaining = SPIN_LIMIT;
        while remaining > 0 {
            if self.status.load(Ordering::Acquire) != NodeStatus::Waiting as u32 {
                return;
            }
            core::hint::spin_loop();
            remaining -= 1;
        }

        if self
            .status
            .compare_exchange(
                NodeStatus::Waiting as u32,
                NodeStatus::Sleeping as u32,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            while self.status.load(Ordering::Relaxed) == NodeStatus::Sleeping as u32 {
                _ = rustix::thread::futex::wait(
                    &self.status,
                    Flags::PRIVATE,
                    NodeStatus::Sleeping as u32,
                    None,
                );
            }
        }
    }

    pub(crate) fn attach(&self, lock: &LambdaLock) -> bool {
        // There is contention for the lock, we need to add our work to the
        // queue of pending work
        let prev = lock
            .tail
            .swap(self as *const Node as *mut Node, Ordering::AcqRel);
        match NonNull::new(prev) {
            Some(prev) => {
                // If we aren't the head, link into predecessor
                unsafe {
                    prev.as_ref()
                        .next
                        .store(self as *const Node as *mut Node, Ordering::Release);
                }
                // Wait for message from predecessor
                self.wait();

                let status = self.status.load(Ordering::Acquire);
                // We were woken up by the predecessor, we can proceed
                if status == NodeStatus::Done as u32 {
                    return true;
                }
                // We were poisoned, we can return false
                if status == NodeStatus::Poisoned as u32 {
                    return false;
                }
            }
            None => {
                // We are the head of the queue. Spin until we acquire the fast path
                // lock.  As we are in the queue future requests shouldn't try to
                // acquire the fast path lock, but stale views of the queue being empty
                // could still be concurrent with this thread.
                while let Err(status) = lock.status.compare_exchange_weak(
                    LockStatus::Unlocked as u32,
                    LockStatus::Locked as u32,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    if status == LockStatus::Locked as u32 {
                        while lock.status.load(Ordering::Relaxed) == LockStatus::Locked as u32 {
                            core::hint::spin_loop();
                        }
                        continue;
                    } else {
                        return false;
                    }
                }
            }
        }
        debug_assert!(lock.status.load(Ordering::Relaxed) == LockStatus::Locked as u32);
        let mut current = NonNull::from_ref(self);
        loop {
            {
                // Install a bomb.
                // If inner task panics, the bomb will be triggered and poison the whole lock.
                let bomb = Bomb {
                    lock: NonNull::from_ref(lock),
                    current,
                };
                unsafe {
                    (current.as_ref().task)(current);
                }
                bomb.defuse();
            }
            let next = unsafe { current.as_ref().next.load(Ordering::Acquire) };
            match NonNull::new(next) {
                Some(next) => {
                    unsafe {
                        // Wake the next node in the queue
                        current.as_ref().wake(NodeStatus::Done);
                    }
                    current = next;
                }
                None => break,
            }
        }
        if lock
            .tail
            .compare_exchange(
                current.as_ptr(),
                core::ptr::null_mut(),
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            unsafe { current.as_ref().wake(NodeStatus::Done) };
            lock.release();
        } else {
            unsafe {
                // Spin until the next node is set
                while current.as_ref().next.load(Ordering::Relaxed).is_null() {
                    core::hint::spin_loop();
                }
                let next = NonNull::new_unchecked(current.as_ref().next.load(Ordering::Acquire));
                next.as_ref().wake(NodeStatus::Head);
                current.as_ref().wake(NodeStatus::Done);
            }
        }
        true
    }
}

impl LambdaLock {
    pub(crate) const fn new() -> Self {
        LambdaLock {
            tail: AtomicPtr::new(core::ptr::null_mut()),
            status: AtomicU32::new(LockStatus::Unlocked as u32),
        }
    }
    pub(crate) fn try_lock(&self) -> Option<bool> {
        if self.tail.load(Ordering::Relaxed).is_null() {
            match self.status.compare_exchange(
                LockStatus::Unlocked as u32,
                LockStatus::Locked as u32,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(true),
                Err(status) => {
                    if status == LockStatus::Locked as u32 {
                        return Some(false);
                    } else if status == LockStatus::Poisoned as u32 {
                        return None;
                    }
                }
            }
        }
        Some(false)
    }
    pub(crate) fn release(&self) {
        self.status
            .store(LockStatus::Unlocked as u32, Ordering::Release);
    }
}
