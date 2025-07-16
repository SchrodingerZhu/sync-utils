use core::{cell::UnsafeCell, ptr::NonNull};

use crate::{PhantomInvariantLifetime, obj::Object};

#[repr(transparent)]
pub struct Flex<'a, T> {
    inner: NonNull<Object<T>>,
    _marker: PhantomInvariantLifetime<'a>,
}

#[repr(transparent)]
pub struct Rigid<T> {
    inner: NonNull<Object<T>>,
}

#[repr(transparent)]
pub struct Field<T> {
    inner: UnsafeCell<NonNull<Object<T>>>,
}

impl<T> Field<T> {
    pub fn new(inner: NonNull<Object<T>>) -> Self {
        Self {
            inner: UnsafeCell::new(inner),
        }
    }
    pub unsafe fn object(&self) -> &Object<T> {
        unsafe { (*self.inner.get()).as_ref() }
    }
}

#[repr(transparent)]
pub struct Nullable<T> {
    inner: UnsafeCell<Option<NonNull<Object<T>>>>,
}

impl<T> Nullable<T> {
    pub fn new_nonnull(inner: NonNull<Object<T>>) -> Self {
        Self {
            inner: UnsafeCell::new(Some(inner)),
        }
    }
    pub fn new_null() -> Self {
        Self {
            inner: UnsafeCell::new(None),
        }
    }
    pub unsafe fn object(&self) -> Option<&Object<T>> {
        let inner = unsafe { &*self.inner.get() };
        inner.as_ref().map(|obj| unsafe { obj.as_ref() })
    }
}
