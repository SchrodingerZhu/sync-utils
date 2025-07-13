use core::{
    ptr::NonNull,
    sync::atomic::{AtomicU32, Ordering},
};

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
    pub fn wait(this: NonNull<Self>, value: u32) {
        #[cfg(not(miri))]
        while unsafe { this.as_ref().load(Ordering::Acquire) == value } {
            while let Err(rustix::io::Errno::INTR) = rustix::thread::futex::wait(
                unsafe { &this.as_ref().0 },
                rustix::thread::futex::Flags::PRIVATE,
                value,
                None,
            ) {
                core::hint::spin_loop();
            }
        }

        #[cfg(miri)]
        while unsafe { this.as_ref().load(Ordering::Acquire) == value } {
            core::hint::spin_loop();
        }
    }

    #[inline(always)]
    pub fn notify(this: NonNull<Self>, new_val: u32, #[allow(unused)] old_val: u32) {
        #[cfg(not(miri))]
        if unsafe { this.as_ref().swap(new_val, Ordering::AcqRel) == old_val } {
            let _ = rustix::thread::futex::wake(
                unsafe { &this.as_ref().0 },
                rustix::thread::futex::Flags::PRIVATE,
                1,
            );
        }

        #[cfg(miri)]
        unsafe {
            this.as_ref().store(new_val, Ordering::Release);
        }
    }
}
