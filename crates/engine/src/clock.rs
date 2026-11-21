//! One monotonic clock domain shared by userspace stamps and kernel receive stamps.
//!
//! * macOS and iOS: `mach_absolute_time` ticks, the same domain the kernel uses for
//!   `SO_TIMESTAMP_MONOTONIC`, converted to nanoseconds.
//! * Linux: `CLOCK_REALTIME` nanoseconds, the domain of `SO_TIMESTAMPNS`.
//! * Windows: `QueryPerformanceCounter` ticks, the domain of `SO_TIMESTAMP` control messages
//!   (`SIO_TIMESTAMPING`), converted to nanoseconds.
//! * Elsewhere: nanoseconds since process start (userspace stamps only).
//!
//! [`awake_total_ns`] reads two other clocks back to back, one that stops while the system sleeps and one that keeps
//! running, so a gap between them shows a sleep:
//! * macOS and iOS: `CLOCK_UPTIME_RAW` (stops) and `CLOCK_MONOTONIC` (runs).
//! * Linux: `CLOCK_MONOTONIC` (stops) and `CLOCK_BOOTTIME` (runs).
//! * Windows: `QueryUnbiasedInterruptTime` (stops) and `QueryPerformanceCounter` (runs).
//! * Elsewhere: [`now_ns`] twice.

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use std::sync::OnceLock;

    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    fn timebase() -> (u64, u64) {
        static TB: OnceLock<(u64, u64)> = OnceLock::new();
        *TB.get_or_init(|| {
            let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
            unsafe { mach_timebase_info(&mut info) };
            if info.denom == 0 { (1, 1) } else { (info.numer as u64, info.denom as u64) }
        })
    }

    #[inline]
    pub fn now_ns() -> u64 {
        ticks_to_ns(unsafe { mach_absolute_time() })
    }

    #[inline]
    pub fn ticks_to_ns(t: u64) -> u64 {
        let (n, d) = timebase();
        if n == d { t } else { ((t as u128 * n as u128) / d as u128) as u64 }
    }

    /// `(awake, total)`: `CLOCK_UPTIME_RAW` stops during sleep, `CLOCK_MONOTONIC` keeps running.
    pub fn awake_total_ns() -> (u64, u64) {
        fn read(clock: libc::clockid_t) -> u64 {
            let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            unsafe { libc::clock_gettime(clock, &mut ts) };
            ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
        }
        (read(libc::CLOCK_UPTIME_RAW), read(libc::CLOCK_MONOTONIC))
    }

    pub const SOURCE: &str = "mach_absolute_time / SO_TIMESTAMP_MONOTONIC";
}

#[cfg(target_os = "linux")]
mod imp {
    #[inline]
    pub fn now_ns() -> u64 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };
        ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
    }

    fn read(clock: libc::clockid_t) -> u64 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(clock, &mut ts) };
        ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
    }

    /// `(awake, total)`: `CLOCK_MONOTONIC` stops during sleep, `CLOCK_BOOTTIME` keeps running.
    pub fn awake_total_ns() -> (u64, u64) {
        (read(libc::CLOCK_MONOTONIC), read(libc::CLOCK_BOOTTIME))
    }

    pub const SOURCE: &str = "CLOCK_REALTIME / SO_TIMESTAMPNS";
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;

    fn frequency() -> u64 {
        static F: OnceLock<u64> = OnceLock::new();
        *F.get_or_init(|| {
            let mut f = 0i64;
            unsafe { QueryPerformanceFrequency(&mut f) };
            f.max(1) as u64
        })
    }

    #[inline]
    pub fn now_ns() -> u64 {
        let mut t = 0i64;
        unsafe { QueryPerformanceCounter(&mut t) };
        ticks_to_ns(t as u64)
    }

    /// Performance-counter ticks (what `SO_TIMESTAMP` carries) to nanoseconds.
    #[inline]
    pub fn ticks_to_ns(t: u64) -> u64 {
        let f = frequency();
        (t / f) * 1_000_000_000 + (t % f) * 1_000_000_000 / f
    }

    /// `(awake, total)`: the unbiased interrupt time stops during sleep, the performance counter keeps running.
    pub fn awake_total_ns() -> (u64, u64) {
        let mut t = 0u64;
        unsafe { QueryUnbiasedInterruptTime(&mut t) };
        // 100 ns units.
        (t.saturating_mul(100), now_ns())
    }

    pub const SOURCE: &str = "QueryPerformanceCounter / SO_TIMESTAMP";
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios", windows)))]
mod imp {
    use std::sync::OnceLock;
    use std::time::Instant;

    #[inline]
    pub fn now_ns() -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }

    /// No sleep-aware clock here: both readings are the same.
    pub fn awake_total_ns() -> (u64, u64) {
        let t = now_ns();
        (t, t)
    }

    pub const SOURCE: &str = "Instant (userspace)";
}

pub use imp::*;

/// Nanoseconds since `t`, an earlier [`now_ns`]. Zero, not a wrapped or panicking subtraction, if the clock stepped
/// back in between (Linux's `CLOCK_REALTIME` can).
#[inline]
pub fn since(t: u64) -> u64 {
    now_ns().saturating_sub(t)
}

/// Ask the scheduler to treat the calling thread as latency-critical, so the gap between
/// the send stamp and the `send()` call stays as short as possible. Best effort: on
/// macOS this is the user-interactive QoS class (performance cores, highest priority
/// without privileges); on Linux a higher priority applies only with CAP_SYS_NICE.
pub fn prioritize_thread() {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
        }
        unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
    }
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn setpriority(which: i32, who: u32, prio: i32) -> i32;
        }
        // PRIO_PROCESS with who = 0 applies to the calling thread on Linux.
        unsafe { setpriority(0, 0, -10) };
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn awake_and_total_advance_together_without_sleep() {
        let (a0, t0) = awake_total_ns();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (a1, t1) = awake_total_ns();
        assert!(a1 > a0 && t1 > t0, "both clocks advance");
        let (da, dt) = ((a1 - a0) as i64, (t1 - t0) as i64);
        assert!((dt - da).abs() < 1_000_000_000, "no sleep gap: awake {da} total {dt}");
    }
}
