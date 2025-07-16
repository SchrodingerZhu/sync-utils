use core::ptr::NonNull;

use alloc::vec::Vec;

use crate::{Flex, Managable, PhantomInvariantLifetime, Rigid, obj::Header};

pub struct Region<'a> {
    pub(crate) allocations: Vec<NonNull<Header>>,
    pub(crate) marker: PhantomInvariantLifetime<'a>,
}

impl Drop for Region<'_> {
    fn drop(&mut self) {
        for alloc in self.allocations.drain(..) {
            Header::drop_unmarked(alloc);
        }
    }
}
impl<'a> Region<'a> {
    pub fn run<F, R>(f: F) -> Rigid<R>
    where
        R: Managable,
        F: for<'r> FnOnce(&mut Region<'r>) -> Flex<'r, R>,
    {
        let mut region = Region {
            allocations: Vec::new(),
            marker: PhantomInvariantLifetime::default(),
        };
        let flex = f(&mut region);
        unsafe { flex.into_rigid() }
    }
}
