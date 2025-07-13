extern crate std;

use crate::pool::VGetrandomOpaqueParams;
use core::ffi::{c_int, c_uint, c_void};
use core::mem::MaybeUninit;
pub type VdsoFunc = unsafe extern "C" fn(*mut c_void, usize, c_uint, *mut c_void, usize) -> c_int;
pub(crate) const PAGE_SIZE: usize = 8 * size_of::<usize>();

unsafe extern "C" fn mocked_vgetrandom(
    buf: *mut c_void,
    buf_len: usize,
    _flags: c_uint,
    udata: *mut c_void,
    udata_len: usize,
) -> c_int {
    if udata_len == usize::MAX {
        let udata = unsafe { &mut (*(udata as *mut MaybeUninit<VGetrandomOpaqueParams>)) };
        unsafe {
            udata.as_mut_ptr().write(VGetrandomOpaqueParams {
                size_of_opaque_states: size_of::<usize>() as u32,
                mmap_prot: 0,
                mmap_flags: 0,
                reserved: [0; 13],
            })
        };
        return 0;
    }
    debug_assert!(udata_len == size_of::<usize>());
    let udata = unsafe { &mut (*(udata as *mut usize)) };
    let buf_slice: &mut [MaybeUninit<u8>] =
        unsafe { core::slice::from_raw_parts_mut(buf as *mut MaybeUninit<u8>, buf_len) };
    for byte in buf_slice.iter_mut() {
        let current = *udata;
        *udata = current.wrapping_add(1);
        byte.write(current as u8);
    }
    return buf_len as c_int;
}

pub fn get_function_and_page_size() -> Option<(VdsoFunc, usize)> {
    Some((mocked_vgetrandom, PAGE_SIZE))
}
