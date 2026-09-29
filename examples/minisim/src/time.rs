use nexosim::time::MonotonicTime;

pub fn current_millis(time: MonotonicTime) -> u64 {
    (time.as_secs() as u64 * 1000) + (time.subsec_nanos() as u64 / 1_000_000)
}
