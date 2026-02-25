// interceptor.rs — macOS global keystroke interception via CGEventTap
//
// Requires the user to grant Accessibility + Input Monitoring in
//   System Settings → Privacy & Security
// The process must NOT be sandboxed (see entitlements.plist).

#![allow(non_upper_case_globals, non_snake_case, improper_ctypes)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Listener};

// ── macOS virtual key codes ───────────────────────────────────────────────────
const kVK_ANSI_Slash: u16 = 0x2C;
const kVK_Delete: u16 = 0x33; // Backspace / Forward-delete
const kVK_ANSI_V: u16 = 0x09;

// ── CGEvent constants ─────────────────────────────────────────────────────────
/// kCGKeyboardEventKeycode field identifier.
const kCGKeyboardEventKeycode: u32 = 9;

/// kCGEventKeyDown type value.
const kCGEventKeyDown: u32 = 10;

/// Bitmask for kCGEventKeyDown used in CGEventTapCreate.
const KEY_DOWN_MASK: u64 = 1 << kCGEventKeyDown;

/// kCGEventFlagMaskCommand — held while posting Cmd+V.
const kCGEventFlagMaskCommand: u64 = 0x0000_0100_0000;

// ── CGEventTap locations ──────────────────────────────────────────────────────
/// kCGHIDEventTap — intercepts hardware events before the window server.
const kCGHIDEventTap: u32 = 0;

/// kCGSessionEventTap — used when *posting* synthetic events so they don't
/// re-trigger our own HID tap.
const kCGSessionEventTap: u32 = 1;

// ── Tap placement / options ───────────────────────────────────────────────────
const kCGHeadInsertEventTap: u32 = 0;
const kCGEventTapOptionDefault: u32 = 0; // active tap (can suppress events)

// ── Raw CoreGraphics / CoreFoundation FFI ────────────────────────────────────
// We use raw FFI instead of the `core-graphics` Rust crate to avoid lifetime
// complications with the static `extern "C"` callback required below.

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// Creates a new CGEventTap.  Returns a retained CFMachPortRef (opaque).
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void,
        userInfo: *mut c_void,
    ) -> *mut c_void;

    /// Reads an integer field from a CGEventRef.
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;

    /// Creates a new key-down or key-up CGEventRef.
    fn CGEventCreateKeyboardEvent(
        source: *mut c_void,
        virtualKey: u16,
        keyDown: bool,
    ) -> *mut c_void;

    /// Overwrites the modifier-flag bits on an event.
    fn CGEventSetFlags(event: *mut c_void, flags: u64);

    /// Posts a CGEvent into the specified event tap stream.
    fn CGEventPost(tapLocation: u32, event: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    /// Creates a CFRunLoopSourceRef from a CFMachPortRef.
    fn CFMachPortCreateRunLoopSource(
        allocator: *mut c_void,
        port: *mut c_void,
        order: isize,
    ) -> *mut c_void;

    /// Adds a CFRunLoopSourceRef to the given run loop under the given mode.
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);

    /// Returns the CFRunLoopRef for the current thread.
    fn CFRunLoopGetCurrent() -> *mut c_void;

    /// Blocks the current thread running the run loop until stopped.
    fn CFRunLoopRun();

    /// Releases a CoreFoundation object.
    fn CFRelease(cf: *mut c_void);

    /// The common modes string — use as the mode argument for CFRunLoopAddSource.
    static kCFRunLoopCommonModes: *const c_void;
}

// ── libdispatch FFI (dispatch to main thread) ────────────────────────────────
#[repr(C)]
struct dispatch_queue_s {
    _opaque: [u8; 0],
}

extern "C" {
    static _dispatch_main_q: dispatch_queue_s;
    fn dispatch_async_f(
        queue: *const dispatch_queue_s,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

// ── Global state (accessed from the static C callback) ───────────────────────

/// The Tauri AppHandle shared with the static callback.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Show the capsule window on the *current* space (including fullscreen)
/// without switching spaces.  Must run on the main thread.
extern "C" fn show_on_main(_ctx: *mut c_void) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
    use tauri::Manager;

    // kCGMaximumWindowLevel (2^31 - 17) — highest possible window level.
    // This guarantees we sit above everything, including the fullscreen
    // app's own window and the system menu bar.
    const MAX_WINDOW_LEVEL: isize = 0x7FFF_FFEF; // 2147483631

    if let Some(handle) = APP_HANDLE.get() {
        if let Some(win) = handle.get_webview_window("main") {
            let ns_win: &NSWindow = unsafe {
                let ptr: *const NSWindow = win.ns_window().unwrap() as *const NSWindow;
                &*ptr
            };

            // 1. Pull the window off screen so macOS "forgets" which space
            //    it was on.  This is required before MoveToActiveSpace works.
            ns_win.orderOut(None);

            // 2. MoveToActiveSpace = macOS moves the window to whatever space
            //    is CURRENTLY active (including a fullscreen space).
            //    Transient      = the window is an ephemeral overlay, not
            //                     bound to any fixed space.
            //    FullScreenAuxiliary = allowed to coexist with a fullscreen
            //                         window on the same space.
            //    IgnoresCycle   = skip Cmd+Tab / Mission Control.
            ns_win.setCollectionBehavior(
                NSWindowCollectionBehavior::MoveToActiveSpace
                    | NSWindowCollectionBehavior::Transient
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );

            // 3. Maximum z-order so we render above the fullscreen app.
            ns_win.setLevel(MAX_WINDOW_LEVEL);

            // 4. Show on the now-current space and accept keyboard input.
            ns_win.orderFrontRegardless();
            ns_win.makeKeyWindow();
        }
    }
}

/// True when the previous non-suppressed key was a '/'.
/// Reset on any non-slash key or when the capsule is triggered.
static SAW_SLASH: AtomicBool = AtomicBool::new(false);

// ── CGEventTap callback ───────────────────────────────────────────────────────

/// Static `extern "C"` function used as the CGEventTap callback.
///
/// State machine for `//` detection:
///
///  IDLE → (/) → SAW_SLASH   [first '/' passes through]
///  SAW_SLASH → (/) → IDLE   [second '/' suppressed, backspace posted, show-capsule emitted]
///  SAW_SLASH → (X) → IDLE   [any other key resets, both events pass through]
extern "C" fn tap_callback(
    _proxy: *mut c_void,
    event_type: u32,
    event: *mut c_void,
    _user_info: *mut c_void,
) -> *mut c_void {
    if event_type != kCGEventKeyDown {
        return event; // pass non-keydown events unchanged
    }

    let keycode =
        unsafe { CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode) } as u16;

    if keycode == kVK_ANSI_Slash {
        // ── second '/' ────────────────────────────────────────────────────────
        if SAW_SLASH.swap(false, Ordering::SeqCst) {
            // Post a backspace to erase the first '/' already delivered to
            // the active app, then suppress this (second) '/' event.
            unsafe {
                let bs_dn =
                    CGEventCreateKeyboardEvent(std::ptr::null_mut(), kVK_Delete, true);
                let bs_up =
                    CGEventCreateKeyboardEvent(std::ptr::null_mut(), kVK_Delete, false);
                // Post to Session tap so we don't re-enter our own HID tap.
                CGEventPost(kCGSessionEventTap, bs_dn);
                CGEventPost(kCGSessionEventTap, bs_up);
                CFRelease(bs_dn);
                CFRelease(bs_up);
            }

            if let Some(handle) = APP_HANDLE.get() {
                // show-capsule → frontend updates React state.
                let _ = handle.emit("show-capsule", ());
            }
            // Show the window on main thread (no space switch).
            unsafe {
                dispatch_async_f(
                    std::ptr::addr_of!(_dispatch_main_q),
                    std::ptr::null_mut(),
                    show_on_main,
                );
            }

            return std::ptr::null_mut(); // suppress the second '/'
        }

        // ── first '/' ─────────────────────────────────────────────────────────
        SAW_SLASH.store(true, Ordering::SeqCst);
        return event; // pass through; will be erased if next key is '/'
    }

    // Any non-slash key resets the trigger state.
    SAW_SLASH.store(false, Ordering::SeqCst);
    event
}

// ── inject_text payload ───────────────────────────────────────────────────────

/// JSON payload shape emitted by the frontend: `{ "text": "..." }`
#[derive(Deserialize)]
struct InjectPayload {
    text: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Boot the interceptor.  Call once from `main.rs` inside `setup()`.
///
/// * Stores the `AppHandle` for use inside the static C callback.
/// * Registers a Tauri event listener for `inject_text` from the frontend.
/// * Spawns a dedicated OS thread that owns the `CGEventTap` + `CFRunLoop`.
pub fn start(app: AppHandle) {
    // Store handle for the static C callback — only set once.
    APP_HANDLE.set(app.clone()).ok();

    // ── inject_text listener ──────────────────────────────────────────────────
    // Frontend emits: await emit('inject_text', { text: 'AI response here' })
    app.listen("inject_text", |event| {
        match serde_json::from_str::<InjectPayload>(event.payload()) {
            Ok(p) => inject_via_paste(&p.text),
            Err(e) => eprintln!("[interceptor] inject_text bad payload: {e}"),
        }
    });

    // ── CGEventTap run loop (dedicated OS thread) ─────────────────────────────
    thread::spawn(|| unsafe {
        // IMPORTANT: delay before calling CGEventTapCreate.
        //
        // CGEventTapCreate triggers a TCC (Input Monitoring) permission check
        // that must interact with the main thread via dispatch_sync.  If this
        // is called while the main thread is still inside tao's
        // `applicationDidFinishLaunching` ObjC callback, the re-entrant main-
        // thread dispatch causes a `panic_cannot_unwind` abort in tao.
        // Waiting 1 s guarantees did_finish_launching has completed and the
        // NSApplication run-loop is fully live before we ask for TCC access.
        thread::sleep(Duration::from_secs(1));

        let tap = CGEventTapCreate(
            kCGHIDEventTap,
            kCGHeadInsertEventTap,
            kCGEventTapOptionDefault,
            KEY_DOWN_MASK,
            tap_callback,
            std::ptr::null_mut(),
        );

        if tap.is_null() {
            // Most common cause: Accessibility permission not granted.
            eprintln!(
                "[interceptor] CGEventTapCreate failed — \
                 grant Accessibility permission in System Settings → Privacy & Security"
            );
            return;
        }

        // Wrap the mach port in a run-loop source and add it to this thread's loop.
        let source = CFMachPortCreateRunLoopSource(std::ptr::null_mut(), tap, 0);
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);

        // Release our own references; the run loop retains what it needs.
        CFRelease(source);
        CFRelease(tap);

        // Block this thread forever, processing HID events.
        CFRunLoopRun();
    });
}

// ── Text injection via clipboard + Cmd+V ─────────────────────────────────────

/// Writes `text` to the macOS clipboard, then posts a synthetic Cmd+V into
/// the session event stream to paste it into whichever app has focus.
fn inject_via_paste(text: &str) {
    match arboard::Clipboard::new() {
        Ok(mut board) => {
            if let Err(e) = board.set_text(text) {
                eprintln!("[interceptor] clipboard write failed: {e}");
                return;
            }
        }
        Err(e) => {
            eprintln!("[interceptor] clipboard init failed: {e}");
            return;
        }
    }

    // Small delay so the clipboard contents are committed before the paste
    // keystroke is received by the target application.
    thread::sleep(Duration::from_millis(60));

    unsafe {
        let v_dn = CGEventCreateKeyboardEvent(std::ptr::null_mut(), kVK_ANSI_V, true);
        let v_up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), kVK_ANSI_V, false);

        CGEventSetFlags(v_dn, kCGEventFlagMaskCommand);
        CGEventSetFlags(v_up, kCGEventFlagMaskCommand);

        CGEventPost(kCGSessionEventTap, v_dn);
        CGEventPost(kCGSessionEventTap, v_up);

        CFRelease(v_dn);
        CFRelease(v_up);
    }
}
