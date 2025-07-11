use std::{collections::HashMap, sync::Mutex};

use criterion::{Criterion, criterion_group, criterion_main};
use lamlock::Lock;

trait Schedule<T>: Sync {
    fn new(value: T) -> Self
    where
        Self: Sized;
    fn schedule<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R + Send,
        R: Send;
}

impl<T: Send> Schedule<T> for Lock<T> {
    fn new(value: T) -> Self {
        Lock::new(value)
    }
    fn schedule<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R + Send,
        R: Send,
    {
        self.run(f).unwrap()
    }
}

impl<T: Send> Schedule<T> for Mutex<T> {
    fn new(value: T) -> Self {
        Mutex::new(value)
    }
    fn schedule<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R + Send,
        R: Send,
    {
        f(&mut self.lock().unwrap())
    }
}

fn integer_add_bench<T: Schedule<i32>>() {
    let lock = T::new(0);
    std::thread::scope(|scope| {
        for _ in 0..128 {
            let lock = &lock;
            scope.spawn(move || {
                lock.schedule(|data| {
                    for i in 0..1000 {
                        *data += i;
                    }
                });
            });
        }
    });
}

fn string_concat_bench<T: Schedule<String>>() {
    let lock = T::new(String::new());
    std::thread::scope(|scope| {
        for _ in 0..128 {
            let lock = &lock;
            scope.spawn(move || {
                lock.schedule(|data| {
                    for i in 0..256 {
                        data.push_str(&i.to_string());
                    }
                });
            });
        }
    });
}

fn hashtable_bench<T: Schedule<HashMap<String, String>>>() {
    let lock = T::new(std::collections::HashMap::new());
    std::thread::scope(|scope| {
        for t in 0..256 {
            let lock = &lock;
            scope.spawn(move || {
                lock.schedule(|data| {
                    for i in 0..2048 {
                        data.insert((t + i).to_string(), i.to_string());
                    }
                });
            });
        }
    });
}

fn initialization<T: Schedule<HashMap<String, String>>>() {
    let lock = T::new(std::collections::HashMap::new());
    std::thread::scope(|scope| {
        for _ in 0..256 {
            let lock = &lock;
            scope.spawn(move || {
                lock.schedule(|data| {
                    if data.len() == 0 {
                        for i in 0..2048 {
                            data.insert((1000000 + i).to_string(), i.to_string());
                        }
                    }
                    for i in 0..2048 {
                        data.insert(i.to_string(), i.to_string());
                    }
                });
            });
        }
    });
}

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("integer add (lamlock)", |b| {
        b.iter(integer_add_bench::<Lock<i32>>)
    });

    c.bench_function("integer add (mutex)", |b| {
        b.iter(integer_add_bench::<Mutex<i32>>)
    });

    c.bench_function("string concat (lamlock)", |b| {
        b.iter(string_concat_bench::<Lock<String>>)
    });

    c.bench_function("string concat (mutex)", |b| {
        b.iter(string_concat_bench::<Mutex<String>>)
    });

    c.bench_function("hashtable (lamlock)", |b| {
        b.iter(hashtable_bench::<Lock<HashMap<String, String>>>)
    });

    c.bench_function("hashtable (mutex)", |b| {
        b.iter(hashtable_bench::<Mutex<HashMap<String, String>>>)
    });

    c.bench_function("initialization (lamlock)", |b| {
        b.iter(initialization::<Lock<HashMap<String, String>>>)
    });

    c.bench_function("initialization (mutex)", |b| {
        b.iter(initialization::<Mutex<HashMap<String, String>>>)
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
