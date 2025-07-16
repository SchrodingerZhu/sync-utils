use core::{cell::Cell, num::NonZero, ptr::NonNull};

use alloc::vec::Vec;

use crate::{Managable, Scanner, VTable};

#[derive(Clone, Copy)]
pub(crate) enum Status {
    Unmarked,
    InProgress,
    Rank(NonZero<usize>),
    Rep(NonNull<Header>),
    Rc(NonZero<usize>),
}

pub(crate) struct Header {
    pub(crate) status: Cell<Status>,
    pub(crate) vtable: &'static VTable,
}

#[repr(C)]
pub(crate) struct Object<T> {
    header: Header,
    value: T,
}

impl Header {
    pub fn new(vtable: &'static VTable) -> Self {
        Self {
            status: Cell::new(Status::Unmarked),
            vtable,
        }
    }
    pub unsafe fn incref(this: NonNull<Self>) {
        let this_ref = unsafe { this.as_ref() };
        if let Status::Rc(rc) = this_ref.status.get() {
            this_ref
                .status
                .set(Status::Rc(unsafe { NonZero::new_unchecked(rc.get() + 1) }));
        }
        unsafe { core::hint::unreachable_unchecked() };
    }
    pub unsafe fn decref(this: NonNull<Self>) -> bool {
        let this_ref = unsafe { this.as_ref() };
        if let Status::Rc(rc) = this_ref.status.get() {
            match rc.get().checked_sub(1).and_then(NonZero::new) {
                Some(x) => this_ref.status.set(Status::Rc(x)),
                None => return true,
            }
            return false;
        }
        unsafe { core::hint::unreachable_unchecked() };
    }
    pub unsafe fn rank(this: NonNull<Self>) -> NonZero<usize> {
        if let Status::Rank(rank) = unsafe { this.as_ref().status.get() } {
            rank
        } else {
            unsafe { core::hint::unreachable_unchecked() }
        }
    }
    pub fn find(mut this: NonNull<Self>) -> NonNull<Self> {
        let mut root = this;
        while let Status::Rep(rep) = unsafe { (*root.as_ptr()).status.get() } {
            root = rep;
        }
        while let Status::Rep(parent) = unsafe { this.as_ref().status.get() } {
            unsafe { this.as_ref().status.set(Status::Rep(root)) };
            this = parent;
        }
        root
    }
    pub fn union(mut r1: NonNull<Self>, mut r2: NonNull<Self>) -> bool {
        r1 = Self::find(r1);
        r2 = Self::find(r2);
        if r1 == r2 {
            return false;
        }
        let rank1 = unsafe { Self::rank(r1) };
        let rank2 = unsafe { Self::rank(r2) };
        if rank1 > rank2 {
            core::mem::swap(&mut r1, &mut r2);
        }
        if rank1 == rank2 {
            unsafe {
                r1.as_ref()
                    .status
                    .set(Status::Rank(NonZero::new_unchecked(rank1.get() + 1)));
            }
        }
        unsafe {
            r2.as_ref().status.set(Status::Rep(r1));
        }
        true
    }
    pub fn freeze_with_worklist(this: NonNull<Self>, worklist: &mut Vec<NonNull<Self>>) {
        match unsafe { Self::find(this).as_ref().status.get() } {
            Status::Unmarked => unsafe {
                this.as_ref()
                    .status
                    .set(Status::Rank(NonZero::new_unchecked(1)));
                worklist.push(this);
                let mut scanner = Scanner::freeze(worklist);
                let vtable = this.as_ref().vtable;
                (vtable.scan)(&mut scanner, this.cast::<u8>());
                if worklist.last().copied() == Some(this) {
                    worklist.pop();
                }
                Self::find(this)
                    .as_ref()
                    .status
                    .set(Status::Rc(NonZero::new_unchecked(1)));
            },
            Status::InProgress => unsafe { core::hint::unreachable_unchecked() },
            Status::Rank(_) => loop {
                let Some(last) = worklist.last().copied() else {
                    break;
                };
                if Self::union(this, last) {
                    worklist.pop();
                } else {
                    break;
                }
            },
            Status::Rep(_) => unsafe {
                core::hint::unreachable_unchecked();
            },
            Status::Rc(_) => unsafe { Self::incref(Self::find(this)) },
        }
    }
    pub fn freeze(this: NonNull<Self>) {
        let mut worklist = Vec::new();
        Self::freeze_with_worklist(this, &mut worklist);
    }
}

impl<T: Managable> Object<T> {
    pub fn new(value: T) -> Self {
        Self {
            header: Header::new(&T::VTABLE),
            value,
        }
    }
    pub fn as_header(&self) -> NonNull<Header> {
        NonNull::from(self).cast()
    }
}
