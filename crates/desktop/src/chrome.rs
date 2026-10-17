//! Keeps the page clear of the window's own chrome on macOS. The page draws its toolbar
//! under a transparent title bar, so it needs to know two things AppKit owns: where the
//! traffic lights end, and in full screen, how far the menu bar and title bar have slid
//! down over the top of the window. Both reach the page as CSS on `<html>`: `--lights`,
//! `--reveal`, and the `fullscreen` and `revealed` classes.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSLayoutAttribute, NSTitlebarAccessoryViewController, NSView, NSWindow, NSWindowButton, NSWindowStyleMask};
use objc2_foundation::{NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration, NSRect, NSString, ns_string};
use tauri::{WebviewWindow, WindowEvent};

struct Ivars {
    window: WebviewWindow,
    /// The last script sent, so a reveal animation doesn't resend unchanged values.
    sent: RefCell<String>,
    /// The last `revealAmount` seen, and whether the title bar is on its way in (or in).
    amount: Cell<f64>,
    shown: Cell<bool>,
}

define_class!(
    // A zero-height title-bar accessory. AppKit animates its `revealAmount` (0 to 1) as the
    // title bar slides in and out in full screen, and sets it as full screen begins and ends.
    #[unsafe(super(NSTitlebarAccessoryViewController))]
    #[thread_kind = MainThreadOnly]
    #[name = "RecursaChromeTracker"]
    #[ivars = Ivars]
    struct Tracker;

    impl Tracker {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(&self, _path: Option<&NSString>, _object: Option<&AnyObject>, _change: Option<&AnyObject>, _context: *mut c_void) {
            sync(&self.ivars().window, false);
            // The title bar window may move just after the value changes: measure again
            // once it has settled.
            let _: () = unsafe { msg_send![self, performSelector: sel!(settle), withObject: None::<&AnyObject>, afterDelay: 0.05f64] };
        }

        #[unsafe(method(settle))]
        fn settle(&self) {
            sync(&self.ivars().window, false);
        }
    }
);

/// Start tracking a main window: its lights now, full-screen changes and reveals from here on.
pub fn install(w: &WebviewWindow) {
    let tw = w.clone();
    let _ = w.run_on_main_thread(move || {
        let (Some(mtm), Some(ns)) = (MainThreadMarker::new(), ns_window(&tw)) else { return };
        let this = Tracker::alloc(mtm).set_ivars(Ivars {
            window: tw.clone(),
            sent: RefCell::default(),
            amount: Cell::new(0.0),
            shown: Cell::new(false),
        });
        let this: Retained<Tracker> = unsafe { msg_send![super(this), init] };
        this.setView(&NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO));
        // Zero-width at the trailing edge: a bottom accessory, even an empty one, makes AppKit
        // lay out a taller title bar and the repositioned traffic lights end up clipped.
        this.setLayoutAttribute(NSLayoutAttribute::Right);
        this.setFullScreenMinHeight(0.0);
        ns.addTitlebarAccessoryViewController(&this);
        // The window owns the tracker and the tracker observes itself, so the observation
        // ends with the window (macOS 10.13+ needs no explicit removal).
        unsafe {
            this.addObserver_forKeyPath_options_context(
                &this,
                ns_string!("revealAmount"),
                NSKeyValueObservingOptions::empty(),
                std::ptr::null_mut(),
            )
        };
    });
    let ew = w.clone();
    w.on_window_event(move |e| {
        if matches!(e, WindowEvent::Resized(_)) {
            sync(&ew, false);
        }
    });
}

/// Send the current state to the page; `force` after a page load, which forgets it.
pub fn sync(w: &WebviewWindow, force: bool) {
    let tw = w.clone();
    let _ = w.run_on_main_thread(move || {
        let Some(ns) = ns_window(&tw) else { return };
        let tracker = tracker(&ns);
        let full = ns.styleMask().contains(NSWindowStyleMask::FullScreen);
        let lights = if full { 0.0 } else { lights(&ns) };
        let reveal = match &tracker {
            Some(t) if full => covered(&ns, t),
            _ => 0.0,
        };
        let js = format!(
            "(e=>{{e.style.setProperty('--lights','{lights:.0}px');e.style.setProperty('--reveal','{reveal:.1}px');\
             e.classList.toggle('fullscreen',{full});e.classList.toggle('revealed',{})}})(document.documentElement)",
            reveal > 0.0
        );
        if let Some(t) = &tracker {
            let mut sent = t.ivars().sent.borrow_mut();
            if !force && *sent == js {
                return;
            }
            sent.clone_from(&js);
        }
        let _ = tw.eval(js);
    });
}

fn ns_window(w: &WebviewWindow) -> Option<Retained<NSWindow>> {
    let ptr = w.ns_window().ok()?.cast::<NSWindow>();
    // SAFETY: Tauri hands out the live NSWindow behind this webview window; retaining it
    // keeps it valid for as long as we hold it.
    unsafe { Retained::retain(ptr) }
}

fn tracker(ns: &NSWindow) -> Option<Retained<Tracker>> {
    ns.titlebarAccessoryViewControllers().iter().find_map(|c| c.downcast::<Tracker>().ok())
}

/// The right edge of the zoom button, in points from the window's left edge.
fn lights(ns: &NSWindow) -> f64 {
    ns.standardWindowButton(NSWindowButton::ZoomButton).map_or(0.0, |b| {
        let r = b.convertRect_toView(b.bounds(), None);
        r.origin.x + r.size.width
    })
}

/// How far the full-screen title bar (and on screens without a notch, the menu bar above
/// it) reaches down over the window once it is in, or 0 while it is out or on its way out.
/// In full screen the accessory lives in the title bar's own window, which keeps its revealed
/// position; `revealAmount` says how much of it shows. The page gets only the destination and
/// eases there itself: the observed steps arrive unevenly, and following each one made the
/// toolbar drift down in hops.
fn covered(ns: &NSWindow, t: &Tracker) -> f64 {
    let amount: f64 = unsafe { msg_send![t, revealAmount] };
    let (last, shown) = (t.ivars().amount.replace(amount), &t.ivars().shown);
    if amount <= 0.0 {
        shown.set(false);
    } else if amount > last {
        shown.set(true);
    } else if amount < last {
        shown.set(false);
    }
    let Some(host) = t.view().window() else { return 0.0 };
    if !shown.get() || std::ptr::eq(&*host, ns) {
        return 0.0;
    }
    let (top, bar) = (ns.frame(), host.frame());
    (top.origin.y + top.size.height - bar.origin.y).clamp(0.0, 160.0)
}
