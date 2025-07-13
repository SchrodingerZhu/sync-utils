use crate::{Error, vdso::VdsoFunc};
use alloc::boxed::Box;
use core::{
    alloc::Layout,
    cell::Cell,
    ffi::{c_int, c_uint, c_void},
    mem::MaybeUninit,
    num::NonZero,
    ptr::NonNull,
    sync::atomic::Ordering,
};
use lamlock::Lock;
use std::pin::Pin;
use syscalls::{Sysno, raw_syscall};

/// This is available in [`linux-raw-sys`]. However, to allow us attempt to use it with unsupported kernels,
/// we define it here.
#[derive(Debug)]
#[repr(C)]
struct VGetrandomOpaqueParams {
    size_of_opaque_states: c_uint,
    mmap_prot: c_uint,
    mmap_flags: c_uint,
    reserved: [c_uint; 13],
}

fn guess_cpu_count() -> NonZero<usize> {
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

#[derive(Debug)]
struct Config {
    pub page_size: usize,
    pub pages_per_block: usize,
    pub states_per_page: usize,
    pub function: VdsoFunc,
    pub params: VGetrandomOpaqueParams,
    pub layout: Layout,
    pub offset: usize,
}

struct BlockHeader {
    prev: Cell<NonNull<Self>>,
    next: Cell<NonNull<Self>>,
}

impl BlockHeader {
    unsafe fn page(&self, offset: usize) -> NonNull<c_void> {
        let this = NonNull::from(self);
        let ptr = unsafe { this.cast::<Cell<NonNull<c_void>>>().byte_add(offset) };
        unsafe { ptr.as_ref().get() }
    }
    unsafe fn freelist(&self, offset: usize, index: usize) -> &Cell<NonNull<c_void>> {
        let this = NonNull::from(self);
        let ptr = unsafe {
            this.cast::<Cell<NonNull<c_void>>>()
                .byte_add(offset)
                .add(index + 1)
        };
        unsafe { ptr.as_ref() }
    }
}

struct AllocGuard {
    layout: Layout,
    ptr: *mut u8,
}
impl Drop for AllocGuard {
    fn drop(&mut self) {
        unsafe {
            alloc::alloc::dealloc(self.ptr, self.layout);
        }
    }
}
struct PageGuard {
    mapped: NonNull<c_void>,
    size: usize,
}
impl Drop for PageGuard {
    fn drop(&mut self) {
        unsafe {
            raw_syscall!(Sysno::munmap, self.mapped.as_ptr(), self.size);
        }
    }
}

impl Config {
    pub fn new() -> Result<Self, Error> {
        let (function, page_size) =
            crate::vdso::get_function_and_page_size().ok_or(Error::NotSupported)?;
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
        let guessed_bytes = guess_cpu_count().get() * params.size_of_opaque_states as usize;
        let aligned_bytes = guessed_bytes + (page_size - (guessed_bytes % page_size));
        let states_per_page = page_size / params.size_of_opaque_states as usize;
        let pages_per_block = aligned_bytes / page_size;
        let array_layout =
            Layout::array::<MaybeUninit<*mut c_void>>(pages_per_block * states_per_page + 1)
                .map_err(|_| Error::AllocationFailure)?;
        let (layout, offset) = Layout::new::<BlockHeader>()
            .extend(array_layout)
            .map_err(|_| Error::AllocationFailure)?;

        Ok(Self {
            page_size,
            pages_per_block,
            states_per_page,
            function,
            params,
            layout,
            offset,
        })
    }

    pub fn allocate_block(&self, sentinel: &BlockHeader) -> Result<NonNull<BlockHeader>, Error> {
        unsafe {
            let raw_memory = alloc::alloc::alloc(self.layout);
            let alloc_guard = AllocGuard {
                layout: self.layout,
                ptr: raw_memory,
            };
            let raw_memory = NonNull::new(raw_memory).ok_or(Error::AllocationFailure)?;
            let mut header = raw_memory.cast::<MaybeUninit<BlockHeader>>();
            let appendix = raw_memory
                .byte_add(self.offset)
                .cast::<MaybeUninit<Cell<NonNull<c_void>>>>();
            let appendix = core::slice::from_raw_parts_mut(
                appendix.as_ptr(),
                self.pages_per_block * self.states_per_page + 1,
            );
            let header_data = BlockHeader {
                prev: Cell::new(sentinel.into()),
                next: Cell::new(sentinel.next.get()),
            };
            header.as_mut().write(header_data);
            let page = raw_syscall!(
                Sysno::mmap,
                core::ptr::null_mut::<c_void>(),
                self.page_size * self.pages_per_block,
                self.params.mmap_prot,
                self.params.mmap_flags,
                -1 as c_int,
                0
            );
            if page as isize == -1 {
                return Err(Error::AllocationFailure);
            }
            let page: NonNull<c_void> =
                NonNull::new(page as *mut c_void).ok_or(Error::AllocationFailure)?;
            let page_guard = PageGuard {
                mapped: page,
                size: self.page_size * self.pages_per_block,
            };
            appendix[0].write(Cell::new(page));
            let mut counter = 1;
            for p in 0..self.pages_per_block {
                let page_ptr = page.byte_add(p * self.page_size);
                for s in 0..self.states_per_page {
                    let state_ptr =
                        page_ptr.byte_add(s * self.params.size_of_opaque_states as usize);
                    appendix[counter].write(Cell::new(state_ptr));
                    counter += 1;
                }
            }
            debug_assert!(counter == appendix.len());
            let header_ref = header.as_ref().assume_init_ref();
            header_ref.next.get().as_ref().prev.set(header_ref.into());
            header_ref.prev.get().as_ref().next.set(header_ref.into());
            core::mem::forget(alloc_guard);
            core::mem::forget(page_guard);
            Ok(header_ref.into())
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct State {
    state: NonNull<c_void>,
    size: usize,
    function: VdsoFunc,
    in_flight: bool,
}

unsafe impl Send for State {}

impl State {
    pub fn fill(&mut self, buf: &mut [u8], flag: c_uint) -> Result<usize, Error> {
        if self.in_flight {
            return Err(Error::Reentrancy);
        }
        self.in_flight = true;
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        let res = unsafe {
            (self.function)(
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                flag,
                self.state.as_ptr() as *mut _,
                self.size,
            )
        };
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        self.in_flight = false;
        if res < 0 {
            return Err(Error::Errno(-res));
        }
        Ok(res as usize)
    }
}

pub struct Pool {
    sentinel: BlockHeader,
    cursor: NonNull<BlockHeader>,
    free_count: usize,
    config: Config,
}

pub struct BoxedPool(Box<Pool>);
impl BoxedPool {
    pub fn new() -> Result<Self, Error> {
        let pool = Pool::new()?;
        Ok(Self(pool))
    }
    pub fn pin_mut(&mut self) -> Pin<&mut Pool> {
        Pin::new(&mut self.0)
    }
}
unsafe impl Send for BoxedPool {}

impl Drop for Pool {
    fn drop(&mut self) {
        let sentinel = NonNull::from(&self.sentinel);
        let mut cursor = self.sentinel.next.get();
        while cursor != sentinel {
            unsafe {
                let _alloc_guard = AllocGuard {
                    layout: self.config.layout,
                    ptr: cursor.cast::<u8>().as_ptr(),
                };
                let _page_guard = PageGuard {
                    mapped: cursor.as_ref().page(self.config.offset),
                    size: self.config.page_size * self.config.pages_per_block,
                };
                cursor = cursor.as_ref().next.get();
            }
        }
    }
}

impl Pool {
    fn new() -> Result<Box<Self>, Error> {
        let config = Config::new()?;
        let sentinel = BlockHeader {
            prev: Cell::new(NonNull::dangling()),
            next: Cell::new(NonNull::dangling()),
        };
        let mut res = Box::new(Self {
            config,
            sentinel,
            cursor: NonNull::dangling(),
            free_count: 0,
        });
        res.cursor = NonNull::from(&res.sentinel);
        res.sentinel.prev.set(res.cursor);
        res.sentinel.next.set(res.cursor);
        Ok(res)
    }
    fn is_locally_full(&self) -> bool {
        self.free_count == self.config.pages_per_block * self.config.states_per_page
    }
    fn is_empty(&self) -> bool {
        self.cursor == NonNull::from(&self.sentinel)
    }
    fn allocate(&mut self) -> Result<(), Error> {
        let block = self.config.allocate_block(&self.sentinel)?;
        self.cursor = block;
        self.free_count = self.config.pages_per_block * self.config.states_per_page;
        Ok(())
    }

    pub fn get(&mut self) -> Result<State, Error> {
        if self.is_empty() {
            self.allocate()?;
            debug_assert!(self.is_locally_full());
        }
        debug_assert!(self.free_count > 0);
        self.free_count -= 1;
        let free_state = unsafe {
            self.cursor
                .as_ref()
                .freelist(self.config.offset, self.free_count)
                .get()
        };
        if self.free_count == 0 {
            self.cursor = unsafe { self.cursor.as_ref().prev.get() };
            self.free_count = self.config.pages_per_block * self.config.states_per_page;
        }
        Ok(State {
            state: free_state,
            function: self.config.function,
            in_flight: false,
            size: self.config.params.size_of_opaque_states as usize,
        })
    }

    pub fn recycle(&mut self, state: State) {
        unsafe {
            if self.is_locally_full() {
                self.cursor = self.cursor.as_ref().next.get();
                self.free_count = 0;
            }
            self.cursor
                .as_ref()
                .freelist(self.config.offset, self.free_count)
                .set(state.state);
            self.free_count += 1;
        }
    }
}

pub struct SharedPool(pub(crate) Lock<BoxedPool>);

impl SharedPool {
    pub fn new() -> Result<Self, Error> {
        let pool = BoxedPool::new()?;
        let lock = Lock::new(pool);
        Ok(Self(lock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn test_guess_cpu_count() {
        if cfg!(miri) {
            return;
        }
        let count = guess_cpu_count();
        assert_eq!(std::thread::available_parallelism().unwrap(), count);
    }

    #[test]
    fn test_config_new() {
        if cfg!(miri) {
            return;
        }
        let config = Config::new().expect("Failed to create Config");
        std::println!("{config:?}");
        assert!(config.page_size > 0, "Page size should be greater than 0");
        assert!(
            config.pages_per_block > 0,
            "Pages per block should be greater than 0"
        );
        assert!(
            config.states_per_page > 0,
            "States per page should be greater than 0"
        );
    }
}
