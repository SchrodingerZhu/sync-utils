#![no_std]
#![feature(ptr_metadata)]
extern crate alloc;

mod obj;
mod pointer;
mod region;
mod scanner;

type PhantomInvariantLifetime<'a> = core::marker::PhantomData<*mut &'a ()>;

pub use pointer::{Field, Flex, Nullable, Rigid};
pub use region::Region;
pub use scanner::{Managable, Scanner};

#[cfg(test)]
mod tests {
    use super::*;
    use crate as refrigerator;
    use alloc::string::String;
    use refrigerator_derive::Managable;

    #[derive(Managable)]
    enum List<T: Managable> {
        Nil,
        Cons(T, #[field] Field<Self>),
    }

    #[test]
    fn test_simple_list() {
        let _rigid = Region::run(|region| {
            let _nil = Flex::new(List::<String>::Nil, region);
            let nil = Flex::new(List::Nil, region);
            let cons = Flex::new(
                List::Cons(String::from("Hello"), nil.into_field(region)),
                region,
            );
            cons
        })
        .clone();
    }

    #[test]
    fn test_nested_region() {
        Region::run(|outer| {
            let rigid = Region::run(|region| {
                let _nil = Flex::new(List::<String>::Nil, region);
                let nil = Flex::new(List::Nil, outer);
                let cons = Flex::new(
                    List::Cons(String::from("Hello"), nil.into_field(outer)),
                    region,
                );
                cons
            });
            let nil = Flex::new(List::Nil, outer);
            let cons = Flex::new(List::Cons(rigid, nil.into_field(outer)), outer);
            cons
        });
    }

    #[derive(Managable)]
    struct Linked<T: Managable>(#[nullable] Nullable<Self>, T, #[nullable] Nullable<Self>);

    // #[test]
    // fn test_linked_list() {
    //     let _rigid = Region::run(|region| {
    //         let nil = Flex::new(Linked::<String>(Nullable::null(), String::from
    //     }
    // }
}
