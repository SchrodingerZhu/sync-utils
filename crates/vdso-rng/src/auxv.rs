use core::{
    ffi::{c_int, c_ulong, c_void},
    mem::{MaybeUninit, size_of},
    ptr::NonNull,
};

use linux_raw_sys::general::{AT_NULL, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE};
use linux_raw_sys::prctl::PR_GET_AUXV;
use syscalls::{Sysno, raw_syscall, syscall};

// Maximum number of entries in the auxiliary vector
// It is a rough guess based on typical usage, but can vary by system.
// 64 should be more than enough for most cases.
const MAX_AUXV_ENTRIES: usize = 64;
const MMAP_SIZE: usize = MAX_AUXV_ENTRIES * size_of::<AuxvEntry>();

#[derive(Clone, Copy)]
#[repr(C)]
pub struct AuxvEntry {
    pub key: c_ulong,
    pub value: c_ulong,
}

pub struct MMappedAuxv {
    ptr: NonNull<MaybeUninit<[AuxvEntry; MAX_AUXV_ENTRIES]>>,
}

impl MMappedAuxv {
    pub fn new() -> Option<Self> {
        unsafe {
            let mmap = raw_syscall!(
                Sysno::mmap,
                core::ptr::null_mut::<c_void>(),
                MMAP_SIZE,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1 as c_int,
                0
            );
            if mmap as isize == -1 {
                return None;
            }
            let mut ptr = NonNull::new(mmap as *mut MaybeUninit<[AuxvEntry; MAX_AUXV_ENTRIES]>)?;
            ptr.as_mut().write(
                [AuxvEntry {
                    key: AT_NULL as c_ulong,
                    value: AT_NULL as c_ulong,
                }; MAX_AUXV_ENTRIES],
            );
            syscall!(Sysno::prctl, PR_GET_AUXV, ptr.as_ptr(), MMAP_SIZE, 0, 0).ok()?;
            Some(MMappedAuxv { ptr })
        }
    }
    pub fn iter(&'_ self) -> AuxvIter<'_> {
        unsafe { AuxvIter::new(self.ptr.as_ref().assume_init_ref()) }
    }
}

pub struct AuxvIter<'a> {
    auxv: &'a [AuxvEntry],
    index: usize,
}

impl<'a> AuxvIter<'a> {
    fn new(auxv: &'a [AuxvEntry]) -> Self {
        AuxvIter { auxv, index: 0 }
    }
}

impl<'a> Iterator for AuxvIter<'a> {
    type Item = AuxvEntry;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.auxv.get(self.index)?;
        if current.key == AT_NULL as c_ulong {
            return None;
        }
        self.index += 1;
        Some(*current)
    }
}

impl Drop for MMappedAuxv {
    fn drop(&mut self) {
        unsafe {
            raw_syscall!(Sysno::munmap, self.ptr.as_ptr() as *mut c_void, MMAP_SIZE);
        }
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use linux_raw_sys::general::AT_RANDOM;

    use super::*;
    #[test]
    fn test_mmap_auxv() {
        let auxv = MMappedAuxv::new();
        assert!(auxv.is_some(), "Failed to mmap auxiliary vector");
    }

    #[test]
    fn test_auxv_iter() {
        let auxv = MMappedAuxv::new().expect("Failed to mmap auxiliary vector");
        assert!(
            auxv.iter().any(|x| x.key == AT_RANDOM as c_ulong),
            "AT_RANDOM not found in auxiliary vector"
        );
    }
}
