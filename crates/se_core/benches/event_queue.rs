use std::hint::black_box;
use std::time::{Duration, Instant};

use se_core::device::DeviceId;
use se_core::event::{EventQueue, ScheduledEvent};

const REARM_ITERATIONS: u32 = 1_000_000;
const SAMPLES: usize = 7;
const EARLY_DEADLINE: u64 = 16_000_000;
const FAR_DEADLINE: u64 = 1_000_000_000;

fn event(vtime: u64, tag: u32) -> ScheduledEvent {
    ScheduledEvent {
        vtime,
        device: DeviceId::from_raw(0),
        tag,
        payload: u64::from(tag),
    }
}

fn buried_rearm_sample() -> Duration {
    let mut queue = EventQueue::new();
    queue.schedule(event(EARLY_DEADLINE, 0)).unwrap();
    let start = Instant::now();
    for tag in 1..=REARM_ITERATIONS {
        let token = queue.schedule(black_box(event(FAR_DEADLINE, tag))).unwrap();
        black_box(queue.cancel(token).unwrap());
        black_box(queue.front_time());
    }
    let elapsed = start.elapsed();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.front_time(), Some(EARLY_DEADLINE));
    black_box(queue);
    elapsed
}

fn top_rearm_sample() -> Duration {
    let mut queue = EventQueue::new();
    let start = Instant::now();
    for tag in 1..=REARM_ITERATIONS {
        let token = queue.schedule(black_box(event(FAR_DEADLINE, tag))).unwrap();
        black_box(queue.cancel(token).unwrap());
        black_box(queue.front_time());
    }
    let elapsed = start.elapsed();
    assert!(queue.is_empty());
    assert_eq!(queue.front_time(), None);
    black_box(queue);
    elapsed
}

fn median_ns(mut sample: impl FnMut() -> Duration) -> (f64, Duration) {
    black_box(sample());
    let mut samples = (0..SAMPLES).map(|_| sample()).collect::<Vec<_>>();
    samples.sort_unstable();
    let median = samples[SAMPLES / 2];
    (
        median.as_secs_f64() * 1e9 / f64::from(REARM_ITERATIONS),
        median,
    )
}

fn report(name: &str, ns_per_rearm: f64, elapsed: Duration) {
    println!("{name:<30} {ns_per_rearm:>9.3} ns/rearm  median {elapsed:?}");
}

fn main() {
    println!("se_core event queue release benchmark");
    println!("samples={SAMPLES} rearm_iterations={REARM_ITERATIONS}");

    let (buried_ns, elapsed) = median_ns(buried_rearm_sample);
    report("buried_cancel_rearm", buried_ns, elapsed);

    let (top_ns, elapsed) = median_ns(top_rearm_sample);
    report("top_cancel_rearm", top_ns, elapsed);
}
