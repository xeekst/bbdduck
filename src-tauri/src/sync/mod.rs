pub mod client;
pub mod engine;
pub mod model;
pub mod protocol;
pub mod server;

/// Worker count for CPU-bound helper pools (parallel scan walk and the
/// incremental skip-check): at least 2, at most half of the system's logical
/// CPUs. Kept modest because these pools are I/O-bound (filesystem stat).
pub(crate) fn half_cpu_workers() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cpus / 2).max(2)
}
