//! One monotonic clock domain shared by userspace stamps and kernel receive stamps.
//!
//! * macOS: `mach_absolute_time` ticks, the same domain the kernel uses for
//!   `SO_TIMESTAMP_MONOTONIC`, converted to nanoseconds.
//! * Linux: `CLOCK_REALTIME` nanoseconds, the domain of `SO_TIMESTAMPNS`.
//! * Windows: `QueryPerformanceCounter` ticks, the domain of `SO_TIMESTAMP` control messages
//!   (`SIO_TIMESTAMPING`), converted to nanoseconds.
//! * Elsewhere: nanoseconds since process start (userspace stamps only).

#[cfg(target_os = "macos")]
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

    pub const SOURCE: &str = "CLOCK_REALTIME / SO_TIMESTAMPNS";
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

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

    pub const SOURCE: &str = "QueryPerformanceCounter / SO_TIMESTAMP";
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    use std::sync::OnceLock;
    use std::time::Instant;

    #[inline]
    pub fn now_ns() -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }

    pub const SOURCE: &str = "Instant (userspace)";
}

pub use imp::*;

/// Ask the scheduler to treat the calling thread as latency-critical, so the gap between
/// the send stamp and the `send()` call stays as short as possible. Best effort: on
/// macOS this is the user-interactive QoS class (performance cores, highest priority
/// without privileges); on Linux a higher priority applies only with CAP_SYS_NICE.
pub fn prioritize_thread() {
    #[cfg(target_os = "macos")]
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
