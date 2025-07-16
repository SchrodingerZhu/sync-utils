#![no_std]
#![feature(ptr_metadata)]
use core::{ffi::c_void, mem::ManuallyDrop, ptr::NonNull};
extern crate alloc;

mod obj;
mod pointer;
mod scanner;

type PhantomInvariantLifetime<'a> = core::marker::PhantomData<*mut &'a ()>;

pub use pointer::{Field, Flex, Nullable, Rigid};
pub use scanner::{Managable, Scanner};
