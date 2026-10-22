//! Run-time monitoring of the heap usage.

use embassy_time::{Duration, Timer};

const PERIOD: Duration = Duration::from_secs(10);
const LOG_HEAP_USAGE: bool = false;
const WARNING_THRESHOLD_PERCENT: usize = 50;
const ERROR_THRESHOLD_PERCENT: usize = 70;
const HYSTERESIS_PERCENT: usize = 5;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum UsageLevel {
    Normal,
    Warning,
    Error,
}

impl UsageLevel {
    fn threshold_percent(self) -> usize {
        match self {
            UsageLevel::Normal => 0,
            UsageLevel::Warning => WARNING_THRESHOLD_PERCENT,
            UsageLevel::Error => ERROR_THRESHOLD_PERCENT,
        }
    }

    /// A level is only left once the usage is clearly below its threshold. Otherwise, a usage
    /// around a threshold would log on every sample.
    fn next(self, used: usize, heap_size: usize) -> Self {
        let percent = used * 100 / heap_size;
        let level = if percent >= ERROR_THRESHOLD_PERCENT {
            UsageLevel::Error
        } else if percent >= WARNING_THRESHOLD_PERCENT {
            UsageLevel::Warning
        } else {
            UsageLevel::Normal
        };
        if level < self && percent + HYSTERESIS_PERCENT > self.threshold_percent() {
            return self;
        }
        level
    }
}

/// The peak is only sampled, so short allocation bursts between two samples are missed.
///
/// TLSF determines the usage by walking all heap blocks inside a critical section, which is why
/// the usage is only read rarely.
pub async fn heap_monitor_task(heap_used: fn() -> usize, heap_size: usize) {
    let mut peak = 0;
    let mut level = UsageLevel::Normal;
    loop {
        let used = heap_used();
        peak = peak.max(used);
        if LOG_HEAP_USAGE {
            defmt::info!(
                "Heap usage: {} of {} bytes, peak {} bytes",
                used,
                heap_size,
                peak
            );
        }
        // Only level changes are logged, so a high usage does not flood the log.
        let new_level = level.next(used, heap_size);
        if new_level != level {
            match new_level {
                UsageLevel::Normal => {
                    defmt::info!("Heap usage back to normal: {} of {} bytes", used, heap_size)
                }
                UsageLevel::Warning => defmt::warn!(
                    "Heap usage above {}%: {} of {} bytes",
                    WARNING_THRESHOLD_PERCENT,
                    used,
                    heap_size
                ),
                UsageLevel::Error => defmt::error!(
                    "Heap usage above {}%: {} of {} bytes",
                    ERROR_THRESHOLD_PERCENT,
                    used,
                    heap_size
                ),
            }
            level = new_level;
        }
        Timer::after(PERIOD).await;
    }
}
