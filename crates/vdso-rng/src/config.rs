use crate::vdso::VdsoFunc;
use core::{ffi::c_uint, mem::MaybeUninit};

#[derive(Debug)]
#[repr(C)]
pub struct VGetrandomOpaqueParams {
    pub size_of_opaque_states: c_uint,
    pub mmap_prot: c_uint,
    pub mmap_flags: c_uint,
    pub reserved: [c_uint; 13],
}

#[derive(Debug)]
pub struct Config {
    pub page_size: usize,
    pub pages_per_block: usize,
    pub states_per_page: usize,
    pub function: VdsoFunc,
    pub params: VGetrandomOpaqueParams,
}

impl Config {
    pub unsafe fn new(function: VdsoFunc, page_size: usize) -> Self {
        let mut params = MaybeUninit::<VGetrandomOpaqueParams>::uninit();
        unsafe {
            function(
                core::ptr::null_mut(),
                0,
                0,
                params.as_mut_ptr() as *mut _,
                !0,
            );
        }
        let params = unsafe { params.assume_init() };
        let guessed_bytes =
            crate::utils::guess_cpu_count().get() * params.size_of_opaque_states as usize;
        let aligned_bytes = guessed_bytes + (page_size - (guessed_bytes % page_size));
        let states_per_page = page_size / params.size_of_opaque_states as usize;
        let pages_per_block = aligned_bytes / page_size;
        Self {
            page_size,
            pages_per_block,
            states_per_page,
            function,
            params,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_config() {
        let (function, page_size) = crate::vdso::get_function_and_page_size().unwrap();
        let config = unsafe { Config::new(function, page_size) };
        assert!(config.page_size > 0);
        assert!(config.pages_per_block > 0);
        assert!(config.states_per_page > 0);
    }
}
