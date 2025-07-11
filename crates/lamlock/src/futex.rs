use core::sync::atomic::{AtomicU32, Ordering};

#[repr(transparent)]
pub struct Futex(AtomicU32);

impl core::ops::Deref for Futex {
    type Target = AtomicU32;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Futex {
    #[inline(always)]
    pub const fn new(value: u32) -> Self {
        Self(AtomicU32::new(value))
    }

    #[inline(always)]
    pub fn wait(&self, value: u32) {
        #[cfg(not(miri))]
        while self.load(Ordering::Acquire) == value {
            while let Err(rustix::io::Errno::INTR) = rustix::thread::futex::wait(
                &self.0,
                rustix::thread::futex::Flags::PRIVATE,
                value,
                None,
            ) {
                core::hint::spin_loop();
            }
        }

        #[cfg(miri)]
        while self.load(Ordering::Acquire) == value {
            core::hint::spin_loop();
        }
    }

    #[inline(always)]
    pub fn notify(&self, new_val: u32, #[allow(unused)] old_val: u32) {
        #[cfg(not(miri))]
        if self.swap(new_val, Ordering::AcqRel) == old_val {
            let _ = rustix::thread::futex::wake(&self.0, rustix::thread::futex::Flags::PRIVATE, 1);
        }

        #[cfg(miri)]
        self.store(new_val, Ordering::Release);
    }
}
