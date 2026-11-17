use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static REALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn recorded_allocation(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
    ALLOCATIONS.fetch_add(1, Relaxed);
    REQUESTED.fetch_add(size, Relaxed);
}

// Every request is forwarded to System unchanged. Counters read only sizes and
// never access allocated memory or allocate from inside a callback.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            recorded_allocation(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            recorded_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            if new_size >= layout.size() {
                let increase = new_size - layout.size();
                let live = LIVE.fetch_add(increase, Relaxed) + increase;
                PEAK.fetch_max(live, Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Relaxed);
            }
            REALLOCATIONS.fetch_add(1, Relaxed);
            REQUESTED.fetch_add(new_size, Relaxed);
        }
        resized
    }
}

pub struct Baseline {
    live: usize,
    allocations: usize,
    reallocations: usize,
    requested: usize,
}

impl Baseline {
    pub fn start() -> Self {
        let live = LIVE.load(Relaxed);
        PEAK.store(live, Relaxed);
        Self {
            live,
            allocations: ALLOCATIONS.load(Relaxed),
            reallocations: REALLOCATIONS.load(Relaxed),
            requested: REQUESTED.load(Relaxed),
        }
    }

    pub fn live_delta(&self) -> i128 {
        LIVE.load(Relaxed) as i128 - self.live as i128
    }

    pub fn peak_delta(&self) -> usize {
        PEAK.load(Relaxed).saturating_sub(self.live)
    }

    pub fn allocations(&self) -> usize {
        ALLOCATIONS.load(Relaxed) - self.allocations
    }

    pub fn reallocations(&self) -> usize {
        REALLOCATIONS.load(Relaxed) - self.reallocations
    }

    pub fn requested(&self) -> usize {
        REQUESTED.load(Relaxed) - self.requested
    }
}
