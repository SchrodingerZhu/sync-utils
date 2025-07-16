use alloc::vec::Vec;

use crate::{
    Field, Rigid,
    obj::{Header, Object, Status},
    pointer::Nullable,
};
use core::ptr::NonNull;
pub struct VTable {
    pub(crate) drop: unsafe fn(NonNull<Header>),
    pub(crate) scan: unsafe fn(&mut Scanner, NonNull<Header>),
}

pub(crate) enum ScannerImpl<'a> {
    Freeze(&'a mut Vec<NonNull<Header>>),
    Dispose {
        scc: &'a mut Vec<NonNull<Header>>,
        dfs: &'a mut Vec<NonNull<Header>>,
    },
}

pub struct Scanner<'a>(ScannerImpl<'a>);

impl<'a> Scanner<'a> {
    pub(crate) fn freeze(worklist: &'a mut Vec<NonNull<Header>>) -> Self {
        Self(ScannerImpl::Freeze(worklist))
    }
    pub(crate) fn dispose(
        scc: &'a mut Vec<NonNull<Header>>,
        dfs: &'a mut Vec<NonNull<Header>>,
    ) -> Self {
        Self(ScannerImpl::Dispose { scc, dfs })
    }
    pub fn scan_nested<T: Managable>(&mut self, value: &T) {
        unsafe {
            value.scan_nested(self);
        }
    }
    pub fn scan_field<T: Managable>(&mut self, field: &Field<T>) {
        self.scan_object(field.object());
    }
    pub fn scan_nullable<T: Managable>(&mut self, nullable: &Nullable<T>) {
        if let Some(object) = unsafe { nullable.object() } {
            self.scan_object(object);
        }
    }
    fn scan_object<T: Managable>(&mut self, object: NonNull<Object<T>>) {
        match &mut self.0 {
            ScannerImpl::Freeze(worklist) => {
                Header::freeze_with_worklist(object.cast(), worklist);
            }
            ScannerImpl::Dispose { scc, dfs } => {
                let n = Header::find(object.cast());
                match unsafe { n.as_ref().status.get() } {
                    Status::InProgress => {
                        if object.cast() != n {
                            Header::add_stack(object.cast(), scc);
                        }
                    }
                    Status::Rc(_) => {
                        if unsafe { Header::decref(n) } {
                            Header::add_stack(n, dfs);
                        }
                    }
                    _ => unsafe { core::hint::unreachable_unchecked() },
                }
            }
        }
    }
}

pub unsafe trait Managable: Sized {
    const VTABLE: VTable = VTable {
        drop: |x: NonNull<Header>| unsafe {
            let object = x.cast::<Object<Self>>();
            let boxed = alloc::boxed::Box::from_raw(object.as_ptr());
            drop(boxed);
        },
        scan: |scanner: &mut Scanner, ptr: NonNull<Header>| unsafe {
            ptr.cast::<Object<Self>>()
                .as_ref()
                .value
                .scan_nested(scanner)
        },
    };
    unsafe fn scan_nested(&self, _: &mut Scanner) {}
}

macro_rules! impl_trivially_managable {
    ($($t:ty)*) => {
        $(unsafe impl Managable for $t {
        })*
    };
}

impl_trivially_managable! {
    ()
    u8 u16 u32 u64 u128 usize
    i8 i16 i32 i64 i128 isize
    f32 f64
    bool char
    alloc::string::String
    alloc::ffi::CString
}

unsafe impl<T: ?Sized> Managable for &'_ T {}
unsafe impl<T: ?Sized> Managable for &'_ mut T {}

unsafe impl<T: Managable, const N: usize> Managable for [T; N] {
    unsafe fn scan_nested(&self, scanner: &mut Scanner) {
        self.iter().for_each(|item| {
            scanner.scan_nested(item);
        });
    }
}

// Rigid is already an RC pointer, so there is nothing to do here.
unsafe impl<T: Managable> Managable for Rigid<T> {
    unsafe fn scan_nested(&self, _: &mut Scanner) {}
}

unsafe impl<T: Managable> Managable for alloc::boxed::Box<T> {
    unsafe fn scan_nested(&self, scanner: &mut Scanner) {
        scanner.scan_nested(&**self);
    }
}

unsafe impl<T: Managable> Managable for alloc::vec::Vec<T> {
    unsafe fn scan_nested(&self, scanner: &mut Scanner) {
        self.iter().for_each(|item| {
            scanner.scan_nested(item);
        });
    }
}

unsafe impl<T: Managable> Managable for alloc::rc::Rc<T> {
    unsafe fn scan_nested(&self, scanner: &mut Scanner) {
        scanner.scan_nested(&**self);
    }
}

unsafe impl<T: Managable> Managable for Option<T> {
    unsafe fn scan_nested(&self, scanner: &mut Scanner) {
        self.iter().for_each(|item| scanner.scan_nested(item));
    }
}
