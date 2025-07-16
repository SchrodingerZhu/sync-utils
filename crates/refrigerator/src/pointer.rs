use core::{cell::Cell, ops::Deref, ptr::NonNull};

use crate::{
    Managable, PhantomInvariantLifetime,
    obj::{Header, Object},
    region::Region,
};

#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Flex<'a, T: Managable> {
    inner: NonNull<Object<T>>,
    marker: PhantomInvariantLifetime<'a>,
}

#[repr(transparent)]
pub struct Rigid<T: Managable> {
    inner: NonNull<Object<T>>,
}

impl<T: Managable> Clone for Rigid<T> {
    fn clone(&self) -> Self {
        let root = Header::find(self.inner.cast());
        unsafe {
            Header::incref(root);
        }
        Self { inner: self.inner }
    }
}

impl<T: Managable> Drop for Rigid<T> {
    fn drop(&mut self) {
        let root = Header::find(self.inner.cast());
        unsafe {
            if Header::decref(root) {
                Header::dispose(root);
            }
        }
    }
}

#[repr(transparent)]
pub struct Field<T: Managable> {
    inner: Cell<NonNull<Object<T>>>,
}

impl<T: Managable> Field<T> {
    pub(crate) fn new(inner: NonNull<Object<T>>) -> Self {
        Self {
            inner: Cell::new(inner),
        }
    }
    pub(crate) fn object(&self) -> NonNull<Object<T>> {
        self.inner.get()
    }
}

#[repr(transparent)]
pub struct Nullable<T: Managable> {
    inner: Cell<Option<NonNull<Object<T>>>>,
}

impl<T: Managable> Nullable<T> {
    pub(crate) fn new_nonnull(inner: NonNull<Object<T>>) -> Self {
        Self {
            inner: Cell::new(Some(inner)),
        }
    }
    pub fn new_null() -> Self {
        Self {
            inner: Cell::new(None),
        }
    }
    pub(crate) unsafe fn object(&self) -> Option<NonNull<Object<T>>> {
        let inner = self.inner.get();
        inner.as_ref().copied()
    }
}

impl<'a, T: Managable> Flex<'a, T> {
    pub fn new(inner: T, token: &mut Region<'a>) -> Flex<'a, T> {
        let alloc = Object::alloc(inner);
        token.allocations.push(alloc.cast());
        Self {
            inner: alloc,
            marker: token.marker,
        }
    }

    pub fn set<F, U>(&self, f: F, val: Flex<'a, U>, _token: &Region<'a>)
    where
        F: FnOnce(&T) -> &Field<U>,
        U: Managable,
    {
        let field = f(unsafe { &self.inner.as_ref().value });
        field.inner.set(val.inner);
    }

    pub fn set_opt<F, U>(&self, f: F, val: Option<Flex<'a, U>>, _token: &Region<'a>)
    where
        F: FnOnce(&T) -> &Nullable<U>,
        U: Managable,
    {
        let nullable = f(unsafe { &self.inner.as_ref().value });
        nullable.inner.set(val.map(|v| v.inner));
    }

    pub fn take_opt<F, U>(&self, f: F, token: &Region<'a>) -> Option<Flex<'a, U>>
    where
        F: FnOnce(&T) -> &Nullable<U>,
        U: Managable,
    {
        let nullable = f(unsafe { &self.inner.as_ref().value });
        nullable.inner.take().map(|inner| Flex {
            inner,
            marker: token.marker,
        })
    }

    pub fn replace_opt<F, U>(
        &self,
        f: F,
        val: Flex<'a, U>,
        _token: &Region<'a>,
    ) -> Option<Flex<'a, U>>
    where
        F: FnOnce(&T) -> &Nullable<U>,
        U: Managable,
    {
        let nullable = f(unsafe { &self.inner.as_ref().value });
        nullable.inner.replace(Some(val.inner)).map(|inner| Flex {
            inner,
            marker: self.marker,
        })
    }

    pub(crate) unsafe fn into_rigid(self) -> Rigid<T> {
        Header::freeze(self.inner.cast());
        Rigid { inner: self.inner }
    }

    pub fn into_field(self, _token: &Region<'a>) -> Field<T> {
        let field = Field::new(self.inner);
        field
    }
    pub fn into_nullable(self, _token: &Region<'a>) -> Nullable<T> {
        let nullable = Nullable::new_nonnull(self.inner);
        nullable
    }
}

impl<'a, T: Managable> Deref for Flex<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &self.inner.as_ref().value }
    }
}

impl<T: Managable> Deref for Rigid<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &self.inner.as_ref().value }
    }
}

impl<T: Managable> Deref for Field<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &self.inner.get().as_ref().value }
    }
}

impl<T: Managable> Nullable<T> {
    pub fn as_ref(&self) -> Option<&T> {
        self.inner.get().map(|ptr| unsafe { &ptr.as_ref().value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate as refrigerator;
    use refrigerator_derive::Managable;

    #[derive(Managable)]
    enum List<T: Managable> {
        Cons(T, #[field] Field<Self>),
        Nil,
    }

    #[test]
    fn test_rigid_list() {
        let first = Object::alloc(List::<i32>::Nil);
        let next = Object::alloc(List::Cons(42, Field::new(first)));
        let header = next.cast();
        Header::freeze(header);
        let rigid = Rigid { inner: next };
        let rigid_clone = rigid.clone();
        drop(rigid);
        drop(rigid_clone);
    }

    #[derive(Managable)]
    enum Tree<T: Managable> {
        Branch(T, #[field] Field<Self>, #[field] Field<Self>),
        Leaf,
    }

    #[test]
    fn test_rigid_tree() {
        let leaf = Object::alloc(Tree::<i32>::Leaf);
        let left = Object::alloc(Tree::Branch(42, Field::new(leaf), Field::new(leaf)));
        let right = Object::alloc(Tree::Branch(42, Field::new(leaf), Field::new(leaf)));
        let root = Object::alloc(Tree::Branch(42, Field::new(left), Field::new(right)));
        let header = root.cast();
        Header::freeze(header);
        let rigid = Rigid { inner: root };
        let rigid_clone = rigid.clone();
        drop(rigid);
        drop(rigid_clone);
    }

    #[derive(Managable)]
    struct Cyclic<T: Managable>(T, #[nullable] Nullable<Self>, #[nullable] Nullable<Self>);

    #[test]
    fn test_rigid_cyclic_all_null() {
        let cyclic = Object::alloc(Cyclic(42, Nullable::new_null(), Nullable::new_null()));
        let header = cyclic.cast();
        Header::freeze(header);
        let rigid = Rigid { inner: cyclic };
        let rigid_clone = rigid.clone();
        drop(rigid);
        drop(rigid_clone);
    }

    #[test]
    fn test_rigid_cyclic_self_ref() {
        let a = Object::alloc(Cyclic(42, Nullable::new_null(), Nullable::new_null()));
        unsafe {
            a.as_ref().value.1.inner.set(Some(a));
            a.as_ref().value.2.inner.set(Some(a));
        }
        let header = a.cast();
        Header::freeze(header);
        let rigid = Rigid { inner: a };
        let rigid_clone = rigid.clone();
        drop(rigid);
        drop(rigid_clone);
    }

    #[test]
    fn test_rigid_cyclic_two_nodes() {
        let a = Object::alloc(Cyclic(42, Nullable::new_null(), Nullable::new_null()));
        let b = Object::alloc(Cyclic(
            42,
            Nullable::new_nonnull(a),
            Nullable::new_nonnull(a),
        ));
        unsafe {
            a.as_ref().value.1.inner.set(Some(b));
            a.as_ref().value.2.inner.set(Some(b));
        }
        let header = b.cast();
        Header::freeze(header);
        let rigid = Rigid { inner: b };
        let rigid_clone = rigid.clone();
        drop(rigid);
        drop(rigid_clone);
    }

    #[test]
    fn test_nested_rigid_cyclic() {
        let inner = {
            let a = Object::alloc(Cyclic(42, Nullable::new_null(), Nullable::new_null()));
            let b = Object::alloc(Cyclic(
                42,
                Nullable::new_nonnull(a),
                Nullable::new_nonnull(a),
            ));
            unsafe {
                a.as_ref().value.1.inner.set(Some(b));
                a.as_ref().value.2.inner.set(Some(b));
            }
            let header = b.cast();
            Header::freeze(header);
            (Rigid { inner: b }).clone()
        };
        let nil = Object::alloc(List::<Rigid<Cyclic<i32>>>::Nil);
        let list = Object::alloc(List::Cons(inner, Field::new(nil)));
        let header = list.cast();
        Header::freeze(header);
        let _ = Rigid { inner: list };
    }

    #[derive(Managable)]
    struct Nested<T: Managable>(Cyclic<T>);

    #[test]
    fn test_plainly_nested_cyclic() {
        let test = Object::alloc(Nested(Cyclic(
            42,
            Nullable::new_null(),
            Nullable::new_null(),
        )));
        let header = test.cast();
        Header::freeze(header);
        let _ = Rigid { inner: test };
    }
}
