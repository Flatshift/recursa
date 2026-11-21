//! Sleep and wake notifications from the OS, for telling a slept computer from a slow one.
//!
//! * macOS: `IORegisterForSystemPower` on a dedicated run-loop thread.
//! * Windows: `PowerRegisterSuspendResumeNotification` with a callback.
//! * Elsewhere: no subscription; the stamps stay 0 and callers fall back to clocks.

use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
#[cfg(any(target_os = "macos", windows))]
use std::sync::atomic::Ordering;

/// When the system last announced sleep and last woke, on the `total` clock of
/// `clock::awake_total_ns` (0 = not seen since the process started).
pub struct Power {
    pub slept_at: AtomicU64,
    pub woke_at: AtomicU64,
}

static POWER: Power = Power { slept_at: AtomicU64::new(0), woke_at: AtomicU64::new(0) };

/// Starts the OS subscription on first call (idempotent) and returns the shared state. A failed registration is silent.
pub fn watch() -> &'static Power {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(start);
    &POWER
}

#[cfg(any(target_os = "macos", windows))]
fn stamp(slot: &AtomicU64) {
    slot.store(crate::clock::awake_total_ns().1, Ordering::Relaxed);
}

#[cfg(not(any(target_os = "macos", windows)))]
fn start() {}

#[cfg(target_os = "macos")]
mod mac {
    use super::{POWER, stamp};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU32, Ordering};

    // iokit_common_msg(x) is 0xE0000000 | x.
    const CAN_SYSTEM_SLEEP: u32 = 0xE000_0270;
    const SYSTEM_WILL_SLEEP: u32 = 0xE000_0280;
    const SYSTEM_HAS_POWERED_ON: u32 = 0xE000_0300;

    type Callback = extern "C" fn(refcon: *mut c_void, service: u32, message_type: u32, message_argument: *mut c_void);

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegisterForSystemPower(refcon: *mut c_void, port: *mut *mut c_void, callback: Callback, notifier: *mut u32) -> u32;
        fn IOAllowPowerChange(kernel_port: u32, notification_id: isize) -> i32;
        fn IONotificationPortGetRunLoopSource(notify: *mut c_void) -> *mut c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopCommonModes: *const c_void;
        fn CFRunLoopGetCurrent() -> *mut c_void;
        fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
        fn CFRunLoopRun();
    }

    /// The root power domain connection `IOAllowPowerChange` needs, set before the run loop starts.
    static ROOT_PORT: AtomicU32 = AtomicU32::new(0);

    extern "C" fn on_power(_refcon: *mut c_void, _service: u32, message_type: u32, message_argument: *mut c_void) {
        let allow = || unsafe { IOAllowPowerChange(ROOT_PORT.load(Ordering::Acquire), message_argument as isize) };
        match message_type {
            // Never veto; sleep is delayed 30 s unless the will-sleep message is acknowledged.
            CAN_SYSTEM_SLEEP => {
                allow();
            }
            SYSTEM_WILL_SLEEP => {
                stamp(&POWER.slept_at);
                allow();
            }
            SYSTEM_HAS_POWERED_ON => stamp(&POWER.woke_at),
            _ => {}
        }
    }

    fn run() {
        let mut port = std::ptr::null_mut();
        let mut notifier = 0u32;
        let root = unsafe { IORegisterForSystemPower(std::ptr::null_mut(), &mut port, on_power, &mut notifier) };
        if root == 0 || port.is_null() {
            return;
        }
        ROOT_PORT.store(root, Ordering::Release);
        unsafe {
            let source = IONotificationPortGetRunLoopSource(port);
            if source.is_null() {
                return;
            }
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
            CFRunLoopRun();
        }
    }

    pub fn start() {
        let _ = std::thread::Builder::new().name("power".into()).spawn(run);
    }
}

#[cfg(target_os = "macos")]
fn start() {
    mac::start();
}

#[cfg(windows)]
fn start() {
    use windows_sys::Win32::System::Power::{DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification};
    use windows_sys::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;

    // Power broadcast types (`PBT_APMSUSPEND`, `PBT_APMRESUMEAUTOMATIC`). Every wake sends the automatic resume; the
    // `PBT_APMRESUMESUSPEND` that follows once someone is at the keyboard would stamp the one wake a second time.
    const SUSPEND: u32 = 4;
    const RESUME_AUTOMATIC: u32 = 0x12;

    unsafe extern "system" fn on_power(_ctx: *const core::ffi::c_void, kind: u32, _setting: *const core::ffi::c_void) -> u32 {
        match kind {
            SUSPEND => stamp(&POWER.slept_at),
            RESUME_AUTOMATIC => stamp(&POWER.woke_at),
            _ => {}
        }
        0
    }

    // The subscription lives as long as the process, so its parameters do too (leaked).
    let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(on_power), Context: core::ptr::null_mut() }));
    let mut handle = core::ptr::null_mut();
    unsafe { PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, params as *mut _ as _, &mut handle) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn watch_is_idempotent() {
        let a = watch();
        let b = watch();
        assert!(std::ptr::eq(a, b));
        let _ = (a.slept_at.load(Ordering::Relaxed), a.woke_at.load(Ordering::Relaxed));
    }
}
