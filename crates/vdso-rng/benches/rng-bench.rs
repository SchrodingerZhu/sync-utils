use criterion::{Criterion, criterion_group, criterion_main};
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};
use std::cell::RefCell;
use vdso_rng::{LocalState, SharedPool};

fn global_pool() -> &'static SharedPool {
    static GLOBAL_STATE: std::sync::LazyLock<SharedPool> =
        std::sync::LazyLock::new(|| SharedPool::new().expect("Failed to create global pool"));
    &GLOBAL_STATE
}
fn fill_vgetrandom(buf: &mut [u8]) {
    thread_local! {
        static LOCAL_STATE: RefCell<LocalState<'static>> =
            RefCell::new(LocalState::new(global_pool()).expect("Failed to create local state"));
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
fn fill_with_rand_chacha20(buf: &mut [u8]) {
    use rand::RngCore;
    use rand::rngs::OsRng;
    use rand::rngs::ReseedingRng;
    use rand_chacha::ChaCha20Core;
    use std::cell::RefCell;

    thread_local! {
        static TLS_RNG: RefCell<ReseedingRng<ChaCha20Core, OsRng>> = RefCell::new(
            ReseedingRng::<ChaCha20Core, _>::new(16 * 1024, OsRng).unwrap()
        );
    }

    TLS_RNG.with(|rng| {
        rng.borrow_mut().fill_bytes(buf);
    });
}

pub fn criterion_benchmark(c: &mut Criterion) {
    {
        const BYTES: usize = 64 * 1024; // 64 KiB
        let mut group = c.benchmark_group("throughput");
        let mut buf = Box::new([0u8; BYTES]) as Box<[u8]>; // 64KiB
        group.bench_function("rand-fill-64KiB-vgetrandom", |b| {
            b.iter(|| {
                fill_vgetrandom(&mut buf);
            });
        });
        group.bench_function("rand-fill-64KiB-getrandom", |b| {
            b.iter(|| {
                fill_getrandom(&mut buf);
            });
        });
        group.bench_function("rand-fill-64KiB-rand-chacha20", |b| {
            b.iter(|| {
                fill_with_rand_chacha20(&mut buf);
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
        group.bench_function("parallel-fill-vgetrandom", |b| {
            b.iter(|| {
                array_of_chunks.par_iter_mut().for_each(|chunk| {
                    fill_vgetrandom(chunk);
                });
            });
        });
        group.bench_function("parallel-fill-getrandom", |b| {
            b.iter(|| {
                array_of_chunks.par_iter_mut().for_each(|chunk| {
                    fill_getrandom(chunk);
                });
            });
        });
        group.bench_function("parallel-fill-rand-chacha20", |b| {
            b.iter(|| {
                array_of_chunks.par_iter_mut().for_each(|chunk| {
                    fill_with_rand_chacha20(chunk);
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
