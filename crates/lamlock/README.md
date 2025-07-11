# LamLock

Ship your critical section instead of data.

`LamLock` provides a MCS-lock style flat-combining lock. Instead of running the critical section
on each thread after acquiring the lock, the head of the waiting queue goes through pending tasks
and runs them for other waiting threads.


## What is an MCS Lock?

An MCS lock is a queue-based lock that is designed to be cache-friendly.
Each thread only spin on its local node, with minimal traffic across threads.
The thread-local node is not nessarily a node inside TLS storage. Rather, the
node can be allocated on stack. It is assumed that, during the lifespan of
the node, the thread is waiting in its locking routine, thus the stack space
is always valid.
```text
--------        ----------------------      ----------------------
| Tail |        |  Thread 1 (Stack)  |      |  Thread 2 (Stack)  |
--------        ----------------------      ----------------------
   |            |  ----------------  | next |                    |
   *-------------> |    Node N    | <---*   |     ..........     |
                |  ----------------  |  |   |   ---------------  | next
                |     ..........     |  *------ |  Node N-1   | <---- ....
                |                    |      |   ---------------  |
                ----------------------      ----------------------
 ┌──────┐        ┌────────────────────┐      ┌────────────────────┐
 │ Tail │        │  Thread 1 (Stack)  │      │  Thread 2 (Stack)  │
 └──┬───┘        ├────────────────────┤      ├────────────────────┤
    │            │  ┌──────────────┐  │ next │                    │
    └────────────┼─>│    Node N    │<─┼──┐   │     ..........     │
                 │  └──────────────┘  │  │   │   ┌─────────────┐  │ next
                 │     ..........     │  └───┼───┤  Node N-1   │<─┼─── ....
                 │                    │      │   └─────────────┘  │
                 └────────────────────┘      └────────────────────┘
```
The queue is processed in a FIFO manner. When submitting a task to the lock
queue, the thread swaps its own node with the global tail pointer to register
itself to the queue. The thread then waits until its own turn. This is
usually signaled by the previous node in the queue. Once the thread receives
the signal, it executes the task and then signals the next node in the queue.

## What is a flat combining lock?

Normally, a Mutex maintains the exclusivity using a lock word. Threads
polls/parks on that lock word until it finds an opportunity to acquire the
lock by a successful posting to the lock word. Then the thread go ahead to do
its task on the shared data. In heavy contention, however, the shared data is
bouncing among threads, causing a lot of extra traffic.
Flat combining is a technique to resolve such problem. The general idea is
that when a thread acquires the lock and finished its critical section, its
cache has the “ownership” of the shared data. Instead of passing such
ownership to the next thread, the current thread can continue to execute the
the crtitical section on behalf of the next thread and signal the next thread
once the task is done.

## How does LamLock work?

We can use the queue itself specifies the job waiting to be done by adding
additional fields to the node such as a function pointer to the critical
section. When a thread acquires the lock on the head of the queue, it begins
to follow the pointers among the nodes and execute their critical sections.
The exclusivity is garanteed by the fact that the combiner thread always
holds a global lock. Such lock is passed to the next combiner if needed.
Instead of let each individual thread to propagate the finishing signal, the
combiner just notify each waiting thread onces their critical section is
executed.

## Is it aware of panics?

Yes, `lamlock` is aware of panics. If a panic occurs during the critical section,
all current head thread and all waiting thread will be notified that the lock is poisoned.
A posioned lock can be recovered by calling `Lock::inspect_poison()`.

## Is it good?

Well, it depends. It is hard for me to come up with a huge benchmark that right now. At
`snmalloc`, a similar design largely improve the startup time of the allocator when there are a lot
of threads trying to claim the lock.

Initial benchmarks show that under simple cases, `lamlock` shows comparable and sometimes better performance:
```text
integer add (lamlock)   time:   [2.1744 ms 2.1947 ms 2.2163 ms]
integer add (mutex)     time:   [2.2152 ms 2.2356 ms 2.2575 ms]

string concat (lamlock) time:   [2.3837 ms 2.4045 ms 2.4263 ms]
string concat (mutex)   time:   [2.3982 ms 2.4184 ms 2.4376 ms]


hashtable (lamlock)     time:   [31.299 ms 31.361 ms 31.422 ms]
hashtable (mutex)       time:   [40.370 ms 40.473 ms 40.587 ms]

initialization (lamlock) time:   [32.982 ms 33.063 ms 33.142 ms]
initialization (mutex)   time:   [42.345 ms 42.418 ms 42.496 ms]
```

It seems that the `nightly` feature (which does node prefetching) and LTO is effective in improving the performance.

## Should I use it?

Again, it depends. Benchmark it before using it.

For example, the following case is very bad for `lamlock`:
```rust
fn integer_add_bench_bad<T: Schedule<i32>>() {
    let lock = Lock::new(0);
    std::thread::scope(|scope| {
        for _ in 0..128 {
            let lock = &lock;
            scope.spawn(move || {
                for i in 0..1000 {
                    lock.run(|data| {
                        *data += i;
                    });
                }
            });
        }
    });
}
```
We get
```text
integer add bad (lamlock) time:   [8.7588 ms 10.712 ms 12.721 ms]
integer add bad (mutex) time:   [3.1807 ms 3.2499 ms 3.3180 ms]
```

This is because the critical section is short, and threads acquire the lock in a tight loop.
Due to the nature of the MCS lock, it is likely that the lock acquisition is interleaved for
the same thread, while it is more likely for the tight loop to be 'squashed' into a continuous run
for the mutex. In this case, the mutex is better.


