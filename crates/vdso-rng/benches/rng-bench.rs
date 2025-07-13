use std::cell::{LazyCell, RefCell};

use criterion::{Criterion, criterion_group, criterion_main};
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};
use vdso_rng::{LocalState, SharedPool};

fn global_pool() -> &'static SharedPool {
    static GLOBAL_STATE: std::sync::LazyLock<SharedPool> =
        std::sync::LazyLock::new(|| SharedPool::new().expect("Failed to create global pool"));
    &GLOBAL_STATE
}
fn fill_vgetrandom(buf: &mut [u8]) {
    thread_local! {
        static LOCAL_STATE: LazyCell<RefCell<LocalState<'static>>> = LazyCell::new(|| {
            RefCell::new(LocalState::new(global_pool()).expect("Failed to create local state"))
        });
    }
    LOCAL_STATE.with(|local_state| {
        let mut state = local_state.borrow_mut();
        state.fill(buf, 0).unwrap();
    });
}
fn fill_getrandom(mut buf: &mut [u8]) {
    while !buf.is_empty() {
        match rustix::rand::getrandom(&mut *buf, rustix::rand::GetRandomFlags::empty()) {
            Ok(len) => {
                buf = &mut buf[len..];
            }
            Err(e) if e == rustix::io::Errno::INTR || e == rustix::io::Errno::AGAIN => {
                // Interrupted, retry
                continue;
            }
            Err(e) => {
                panic!("getrandom failed: {e}");
            }
        }
    }
}
fn fill_with_thread_rng(buf: &mut [u8]) {
    use rand::RngCore;
    let mut rng = rand::rngs::ThreadRng::default();
    rng.fill_bytes(buf);
}

pub fn criterion_benchmark(c: &mut Criterion) {
    {
        const REPEAT: usize = 1024; // Number of iterations for each benchmark
        const BYTES: usize = 64 * 1024; // 64 KiB
        let mut group = c.benchmark_group("throughput");
        let mut buf = Box::new([0u8; BYTES]) as Box<[u8]>; // 64KiB
        group.bench_function("rand-fill-64KiB-vgetrandom", |b| {
            b.iter(|| {
                for _ in 0..REPEAT {
                    // Fill the buffer with random bytes using vgetrandom
                    fill_vgetrandom(&mut buf);
                }
            });
        });
        group.bench_function("rand-fill-64KiB-getrandom", |b| {
            b.iter(|| {
                for _ in 0..REPEAT {
                    fill_getrandom(&mut buf);
                }
            });
        });
        group.bench_function("rand-fill-64KiB-thread_rng", |b| {
            b.iter(|| {
                for _ in 0..REPEAT {
                    fill_with_thread_rng(&mut buf);
                }
            });
        });
        group.throughput(criterion::Throughput::Bytes(BYTES as u64));
        group.finish();
    }
    {
        const TOTAL_CHUNKS: usize = 1024 * 1024 * 8;
        const CHUNK_SIZE: usize = 8;
        let mut array_of_chunks = vec![[0u8; CHUNK_SIZE]; TOTAL_CHUNKS];
        let mut group = c.benchmark_group("rayon");
        group.bench_function("parallel-fill-1024bytes-vgetrandom", |b| {
            b.iter(|| {
                array_of_chunks.par_iter_mut().for_each(|chunk| {
                    fill_vgetrandom(chunk);
                });
            });
        });
        group.bench_function("parallel-fill-1024bytes-getrandom", |b| {
            b.iter(|| {
                array_of_chunks.par_iter_mut().for_each(|chunk| {
                    fill_getrandom(chunk);
                });
            });
        });
        group.bench_function("parallel-fill-1024bytes-thread_rng", |b| {
            b.iter(|| {
                array_of_chunks.par_iter_mut().for_each(|chunk| {
                    fill_with_thread_rng(chunk);
                });
            });
        });
        group.throughput(criterion::Throughput::Bytes(
            (TOTAL_CHUNKS * CHUNK_SIZE) as u64,
        ));
        group.finish();
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
