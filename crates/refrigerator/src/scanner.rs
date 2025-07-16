use alloc::vec::Vec;

use crate::{
    Field, Flex, Rigid,
    obj::{Header, Object},
    pointer::Nullable,
};
use core::ptr::NonNull;
pub struct VTable {
    pub drop: unsafe fn(NonNull<u8>),
    pub scan: unsafe fn(&mut Scanner, NonNull<u8>),
}

pub(crate) enum ScannerImpl<'a> {
    Freeze(&'a mut Vec<NonNull<Header>>),
    Dispose,
}

pub struct Scanner<'a>(ScannerImpl<'a>);

impl<'a> Scanner<'a> {
    pub(crate) fn freeze(worklist: &'a mut Vec<NonNull<Header>>) -> Self {
        Self(ScannerImpl::Freeze(worklist))
    }
    pub fn scan_nested<T: Managable>(&mut self, value: &T) {
        unsafe {
            value.scan_nested(self);
        }
    }
    pub fn scan_field<T: Managable>(&mut self, field: &Field<T>) {
        self.scan_object(unsafe { field.object() });
    }
    pub fn scan_nullable<T: Managable>(&mut self, nullable: &Nullable<T>) {
        if let Some(object) = unsafe { nullable.object() } {
            self.scan_object(object);
        }
    }
    fn scan_object<T: Managable>(&mut self, object: &Object<T>) {
        match &mut self.0 {
            ScannerImpl::Freeze(worklist) => {
                Header::freeze_with_worklist(object.as_header(), *worklist);
            }
            ScannerImpl::Dispose => {
                todo!("Implement dispose logic for Scanner");
            }
        }
    }
}

pub unsafe trait Managable: Sized {
    const VTABLE: VTable = VTable {
        drop: |x: NonNull<u8>| unsafe { x.cast::<Self>().drop_in_place() },
        scan: |scanner: &mut Scanner, ptr: NonNull<u8>| unsafe {
            ptr.cast::<Self>().as_ref().scan_nested(scanner)
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

// unsafe impl<T: Scannable + ?Sized> Scannable for alloc::boxed::Box<T> {
//     type Rigid = alloc::boxed::Box<T::Rigid>;
//     type Flex = alloc::boxed::Box<T::Flex>;
//     fn scan(&self, scanner: &mut Scanner) {
//         scanner.scan(self.as_ref());
//     }
// }

// unsafe impl<T: Scannable + ?Sized> Scannable for alloc::vec::Vec<T> {
//     type Rigid = alloc::vec::Vec<T::Rigid>;
//     type Flex = alloc::vec::Vec<T::Flex>;
//     fn scan(&self, scanner: &mut Scanner) {
//         self.iter().for_each(|item| scanner.scan(item));
//     }
// }

// unsafe impl<T: Scannable + ?Sized> Scannable for alloc::rc::Rc<T> {
//     type Rigid = alloc::rc::Rc<T::Rigid>;
//     type Flex = alloc::rc::Rc<T::Flex>;
//     fn scan(&self, scanner: &mut Scanner) {
//         scanner.scan(self.as_ref());
//     }
// }

// unsafe impl<T: Scannable> Scannable for Option<T> {
//     type Rigid = Option<T::Rigid>;
//     type Flex = Option<T::Flex>;
//     fn scan(&self, scanner: &mut Scanner) {
//         self.iter().for_each(|item| scanner.scan(item));
//     }
// }
