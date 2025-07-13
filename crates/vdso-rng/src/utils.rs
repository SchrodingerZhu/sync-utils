#[allow(unused_imports)]
use core::{
    ffi::{c_int, c_uint, c_void},
    num::NonZero,
    ptr::NonNull,
};
#[allow(unused_imports)]
use syscalls::{Sysno, raw_syscall};

#[cfg(not(miri))]
pub fn guess_cpu_count() -> NonZero<usize> {
    let mut cpu_set = [0u8; 128];
    let res = unsafe {
        raw_syscall!(
            Sysno::sched_getaffinity,
            0,
            cpu_set.len(),
            cpu_set.as_mut_ptr()
        )
    } as isize;
    let one = unsafe { NonZero::new_unchecked(1) };
    if res <= 0 {
        return one;
    }
    NonZero::new(
        cpu_set
            .iter()
            .map(|x| x.count_ones() as usize)
            .sum::<usize>(),
    )
    .unwrap_or(one)
}

#[cfg(miri)]
pub fn guess_cpu_count() -> NonZero<usize> {
    NonZero::new(4).unwrap()
}

#[cfg(not(miri))]
pub fn mmap(size: usize, mmap_prot: c_uint, mmap_flags: c_uint) -> Option<NonNull<c_void>> {
    let addr = unsafe { raw_syscall!(Sysno::mmap, 0, size, mmap_prot, mmap_flags, -1 as c_int, 0) };
    if addr as isize == -1 {
        return None;
    }
    NonNull::new(addr as *mut c_void)
}

#[cfg(miri)]
pub fn mmap(size: usize, _mmap_prot: c_uint, _mmap_flags: c_uint) -> Option<NonNull<c_void>> {
    extern crate alloc;
    let layout = core::alloc::Layout::from_size_align(size, crate::vdso::PAGE_SIZE)
        .expect("Failed to create layout for mmap");
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) } as *mut c_void;
    NonNull::new(ptr)
}

#[cfg(not(miri))]
pub fn munmap(ptr: NonNull<c_void>, size: usize) {
    unsafe { raw_syscall!(Sysno::munmap, ptr.as_ptr(), size) };
}

#[cfg(miri)]
pub fn munmap(ptr: NonNull<c_void>, size: usize) {
    extern crate alloc;
    let layout = core::alloc::Layout::from_size_align(size, crate::vdso::PAGE_SIZE)
        .expect("Failed to create layout for munmap");
    unsafe { alloc::alloc::dealloc(ptr.as_ptr() as *mut u8, layout) };
}
