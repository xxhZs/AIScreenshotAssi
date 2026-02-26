// interceptor.rs — macOS global keystroke interception via CGEventTap
//
// Requires the user to grant Accessibility + Input Monitoring in
//   System Settings → Privacy & Security
// The process must NOT be sandboxed (see entitlements.plist).

#![allow(non_upper_case_globals, non_snake_case, improper_ctypes)]

use std::ffi::c_void;
use std::ffi::c_char;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use std::process::Command;

use objc2::AnyThread;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use objc2_foundation::{NSArray, NSNumber};
use objc2_foundation::NSString;
use serde::Deserialize;
use serde::Serialize;
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
// Correct value is (1 << 20) = 0x0010_0000.
// If this is wrong, the target app will receive a literal "v" instead of Cmd+V.
const kCGEventFlagMaskCommand: u64 = 0x0010_0000;

// kCGScrollEventUnitPixel
const kCGScrollEventUnitPixel: u32 = 0;

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

    /// Sets a Unicode string for a key-down keyboard event.
    fn CGEventKeyboardSetUnicodeString(event: *mut c_void, stringLength: usize, unicodeString: *const u16);

    /// Non-variadic scroll wheel event creator (macOS 10.13+).
    fn CGEventCreateScrollWheelEvent2(
        source: *mut c_void,
        units: u32,
        wheelCount: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> *mut c_void;

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

    fn CFRetain(cf: *const c_void) -> *const c_void;

    /// The common modes string — use as the mode argument for CFRunLoopAddSource.
    static kCFRunLoopCommonModes: *const c_void;

    fn CFGetTypeID(cf: *const c_void) -> usize;

    fn CFStringGetTypeID() -> usize;
    fn CFStringGetLength(theString: *const c_void) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        theString: *const c_void,
        buffer: *mut c_char,
        bufferSize: isize,
        encoding: u32,
    ) -> bool;

    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(boolean: *const c_void) -> bool;

    fn CFAttributedStringGetTypeID() -> usize;
    fn CFAttributedStringGetString(theString: *const c_void) -> *const c_void;

    fn CFStringCreateWithCString(
        alloc: *const c_void,
        cStr: *const c_char,
        encoding: u32,
    ) -> *const c_void;

    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(theArray: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(theArray: *const c_void, idx: isize) -> *const c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> *mut c_void;
    fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
    fn AXUIElementCopyAttributeValue(
        element: *mut c_void,
        attribute: *const c_void,
        value: *mut *mut c_void,
    ) -> i32;

    fn AXUIElementPerformAction(element: *mut c_void, action: *const c_void) -> i32;
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

// ── Private Space APIs (best-effort) ─────────────────────────────────────────
//
// On macOS 15, AppKit window collection behaviors are sometimes insufficient
// for reliably showing above *other apps'* fullscreen Spaces. Many launcher
// tools (Raycast/Alfred) rely on private WindowServer APIs to attach a window
// to the currently active Space.
//
// We don't link against private frameworks; instead we resolve symbols at
// runtime using `dlsym`. If not available, we fall back to AppKit behaviors.

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_DEFAULT: *mut c_void = (-2isize) as *mut c_void;

type CGSConnectionID = u32;
type CGSSpaceID = u64;

type MainConnectionFn = unsafe extern "C" fn() -> CGSConnectionID;
type GetActiveSpaceFn = unsafe extern "C" fn(CGSConnectionID) -> CGSSpaceID;
type AddWindowsToSpacesFn =
    unsafe extern "C" fn(CGSConnectionID, *const NSArray<NSNumber>, *const NSArray<NSNumber>) -> i32;

unsafe fn load_fn<T>(name: &'static [u8]) -> Option<T> {
    let ptr = dlsym(RTLD_DEFAULT, name.as_ptr() as *const c_char);
    if ptr.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&ptr))
    }
}

fn try_attach_window_to_active_space(ns_win: &objc2_app_kit::NSWindow) -> bool {
    static ATTACH_ERROR_LOGGED: AtomicBool = AtomicBool::new(false);

    let main_conn: Option<MainConnectionFn> = unsafe {
        load_fn(b"SLSMainConnectionID\0").or_else(|| load_fn(b"CGSMainConnectionID\0"))
    };
    let get_active_space: Option<GetActiveSpaceFn> = unsafe {
        load_fn(b"SLSGetActiveSpace\0").or_else(|| load_fn(b"CGSGetActiveSpace\0"))
    };
    let add_windows_to_spaces: Option<AddWindowsToSpacesFn> = unsafe {
        load_fn(b"SLSAddWindowsToSpaces\0").or_else(|| load_fn(b"CGSAddWindowsToSpaces\0"))
    };

    let (Some(main_conn), Some(get_active_space), Some(add_windows_to_spaces)) =
        (main_conn, get_active_space, add_windows_to_spaces)
    else {
        eprintln!("[interceptor] Space attach: private symbols not available; falling back");
        return false;
    };

    let conn = unsafe { main_conn() };
    let active_space = unsafe { get_active_space(conn) };

    let win_id = ns_win.windowNumber();
    if win_id <= 0 {
        return false;
    }

    let win_num = NSNumber::numberWithInteger(win_id);
    let space_num = NSNumber::numberWithUnsignedLongLong(active_space);

    let windows: objc2::rc::Retained<NSArray<NSNumber>> = NSArray::arrayWithObject(&*win_num);
    let spaces: objc2::rc::Retained<NSArray<NSNumber>> = NSArray::arrayWithObject(&*space_num);

    let rc = unsafe {
        add_windows_to_spaces(
            conn,
            (&*windows) as *const NSArray<NSNumber>,
            (&*spaces) as *const NSArray<NSNumber>,
        )
    };
    if rc != 0 {
        if !ATTACH_ERROR_LOGGED.swap(true, Ordering::Relaxed) {
            eprintln!("[interceptor] Space attach: add_windows_to_spaces failed (rc={rc}); falling back");
        }
    }
    rc == 0
}

// ── Global state (accessed from the static C callback) ───────────────────────

/// The Tauri AppHandle shared with the static callback.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// The app that was frontmost when the capsule was triggered (bundle identifier).
static LAST_TARGET_BUNDLE_ID: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
pub struct ContextSnapshot {
    pub bundle_id: Option<String>,
    pub app_name: Option<String>,
    pub pid: Option<i32>,
    pub clipboard_text: Option<String>,
    pub window_title: Option<String>,
    pub selected_text: Option<String>,
    pub focused_role: Option<String>,
    pub has_text_caret: Option<bool>,
    pub full_page_text: Option<String>,
    pub full_page_method: Option<String>,
    pub full_page_error: Option<String>,
    pub screenshot_path: Option<String>,
    #[serde(default)]
    pub screenshot_paths: Option<Vec<String>>,
    /// Screenshot capture strategy: `"single"` or `"scroll"` (best-effort).
    pub screenshot_capture_kind: Option<String>,
    /// If scroll capture was used: `"up"`, `"down"`, or `"both"`.
    pub screenshot_scroll_direction: Option<String>,
    /// If scroll capture was used: number of extra captures attempted.
    pub screenshot_scroll_pages: Option<u32>,
    /// If scroll capture was used: scroll delta in pixels between captures (approx).
    pub screenshot_scroll_pixels: Option<u32>,
    pub ocr_text: Option<String>,
}

static LAST_CONTEXT_SNAPSHOT: OnceLock<Mutex<Option<ContextSnapshot>>> = OnceLock::new();

#[derive(Clone, Copy)]
struct AxUiElement(*mut c_void);
unsafe impl Send for AxUiElement {}
unsafe impl Sync for AxUiElement {}

static LAST_TARGET_AX_WINDOW: OnceLock<Mutex<Option<AxUiElement>>> = OnceLock::new();

fn store_last_target_ax_window(win: *mut c_void) {
    if win.is_null() {
        return;
    }
    let store = LAST_TARGET_AX_WINDOW.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = store.lock() {
        if let Some(old) = guard.take() {
            unsafe { CFRelease(old.0) };
        }
        let retained = unsafe { CFRetain(win as *const c_void) } as *mut c_void;
        *guard = Some(AxUiElement(retained));
    }
}

fn raise_last_target_window() {
    let store = LAST_TARGET_AX_WINDOW.get_or_init(|| Mutex::new(None));
    let win = match store.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    };
    let Some(win) = win else { return; };

    let action = std::ffi::CString::new("AXRaise").ok();
    let Some(action) = action else { return; };
    let cf_action = unsafe {
        CFStringCreateWithCString(std::ptr::null(), action.as_ptr(), kCFStringEncodingUTF8)
    };
    if cf_action.is_null() {
        return;
    }
    let _ = unsafe { AXUIElementPerformAction(win.0, cf_action) };
    unsafe { CFRelease(cf_action as *mut c_void) };
}

pub fn last_context_snapshot() -> Option<ContextSnapshot> {
    let store = LAST_CONTEXT_SNAPSHOT.get_or_init(|| Mutex::new(None));
    match store.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    }
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

const kCFStringEncodingUTF8: u32 = 0x0800_0100;

fn cf_string_to_rust(cf: *const c_void) -> Option<String> {
    if cf.is_null() {
        return None;
    }
    let len = unsafe { CFStringGetLength(cf) };
    if len <= 0 {
        return Some(String::new());
    }
    let max = unsafe { CFStringGetMaximumSizeForEncoding(len, kCFStringEncodingUTF8) };
    if max <= 0 {
        return None;
    }
    let mut buf = vec![0u8; (max as usize) + 1];
    let ok = unsafe {
        CFStringGetCString(
            cf,
            buf.as_mut_ptr() as *mut c_char,
            buf.len() as isize,
            kCFStringEncodingUTF8,
        )
    };
    if !ok {
        return None;
    }
    let nul = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..nul].to_vec()).ok()
}

fn cf_type_to_string(cf: *const c_void) -> Option<String> {
    if cf.is_null() {
        return None;
    }
    let tid = unsafe { CFGetTypeID(cf) };
    if tid == unsafe { CFStringGetTypeID() } {
        return cf_string_to_rust(cf);
    }
    if tid == unsafe { CFAttributedStringGetTypeID() } {
        let inner = unsafe { CFAttributedStringGetString(cf) };
        return cf_string_to_rust(inner);
    }
    None
}

fn cf_type_to_bool(cf: *const c_void) -> Option<bool> {
    if cf.is_null() {
        return None;
    }
    let tid = unsafe { CFGetTypeID(cf) };
    if tid == unsafe { CFBooleanGetTypeID() } {
        return Some(unsafe { CFBooleanGetValue(cf) });
    }
    None
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn ax_copy_attr(element: *mut c_void, attr: *const c_void) -> Option<*mut c_void> {
    if element.is_null() || attr.is_null() {
        return None;
    }
    let mut out: *mut c_void = std::ptr::null_mut();
    let rc = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut out) };
    if rc == 0 && !out.is_null() {
        Some(out)
    } else {
        None
    }
}

fn ax_copy_attr_name(element: *mut c_void, name: &str) -> Option<*mut c_void> {
    let cstr = std::ffi::CString::new(name).ok()?;
    let cf_attr =
        unsafe { CFStringCreateWithCString(std::ptr::null(), cstr.as_ptr(), kCFStringEncodingUTF8) };
    if cf_attr.is_null() {
        return None;
    }
    let out = ax_copy_attr(element, cf_attr);
    unsafe { CFRelease(cf_attr as *mut c_void) };
    out
}

fn capture_accessibility_context(snapshot: &mut ContextSnapshot) {
    // Best-effort: requires Accessibility permission (already needed for CGEventTap).
    let sys = unsafe { AXUIElementCreateSystemWide() };
    if sys.is_null() {
        return;
    }

    // Selected text / focused value.
    if let Some(focused) = ax_copy_attr_name(sys, "AXFocusedUIElement") {
        // Role / editable detection (best-effort "caret present" heuristic).
        let role = ax_copy_attr_name(focused, "AXRole")
            .and_then(|v| {
                let s = cf_type_to_string(v).map(|s| s.trim().to_string());
                unsafe { CFRelease(v) };
                s
            });
        snapshot.focused_role = role.clone();

        let editable = ax_copy_attr_name(focused, "AXEditable")
            .and_then(|v| {
                let b = cf_type_to_bool(v);
                unsafe { CFRelease(v) };
                b
            })
            .unwrap_or(false);

        let has_range_attr = ax_copy_attr_name(focused, "AXSelectedTextRange")
            .map(|v| {
                unsafe { CFRelease(v) };
                true
            })
            .unwrap_or(false);

        // Heuristic: "is there a place where typing would land in the frontmost app?"
        // Some apps don't reliably expose AXEditable/AXSelectedTextRange even when a caret exists,
        // so we treat common text-entry roles as caret-present on their own.
        let role_typing_surface = matches!(
            role.as_deref(),
            Some("AXTextField") | Some("AXTextArea") | Some("AXSearchField") | Some("AXComboBox")
        );

        snapshot.has_text_caret = Some(has_range_attr || editable || role_typing_surface);

        if let Some(sel) = ax_copy_attr_name(focused, "AXSelectedText") {
            snapshot.selected_text = cf_type_to_string(sel).map(|s| s.trim().to_string());
            unsafe { CFRelease(sel) };
        }
        if snapshot
            .selected_text
            .as_ref()
            .map(|s| s.is_empty())
            .unwrap_or(true)
        {
            if let Some(val) = ax_copy_attr_name(focused, "AXValue") {
                snapshot.selected_text = cf_type_to_string(val).map(|s| s.trim().to_string());
                unsafe { CFRelease(val) };
            }
        }
        unsafe { CFRelease(focused) };
    }

    // Window title.
    if let Some(app) = ax_copy_attr_name(sys, "AXFocusedApplication") {
        if let Some(win) = ax_copy_attr_name(app, "AXFocusedWindow") {
            if let Some(title) = ax_copy_attr_name(win, "AXTitle") {
                snapshot.window_title = cf_type_to_string(title).map(|s| s.trim().to_string());
                unsafe { CFRelease(title) };
            }
            // Store the focused window so we can raise it again before injection.
            store_last_target_ax_window(win);
            unsafe { CFRelease(win) };
        }
        unsafe { CFRelease(app) };
    }

    unsafe { CFRelease(sys) };

    // Truncate to keep prompt bounded.
    if let Some(t) = snapshot.selected_text.as_mut() {
        if t.len() > 6000 {
            *t = format!("{}…", truncate_utf8(t, 6000));
        }
    }
    if let Some(t) = snapshot.window_title.as_mut() {
        if t.len() > 400 {
            *t = format!("{}…", truncate_utf8(t, 400));
        }
    }
}

fn try_capture_full_page_text(snapshot: &mut ContextSnapshot) {
    static FULLPAGE_ERROR_LOGGED: AtomicBool = AtomicBool::new(false);

    let Some(bundle_id) = snapshot.bundle_id.as_deref() else {
        return;
    };

    // Default behavior: try full-page capture for supported browsers unless explicitly disabled.
    // This requires Automation permission on first use.
    let enabled = match std::env::var("DARLING_CTX_FULLPAGE") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => true,
    };
    if !enabled {
        return;
    }

    fn chromium_script(app_name: &str) -> String {
        // Most Chromium browsers support AppleScript with the same surface as Chrome:
        // - front window → active tab
        // - execute ... javascript
        format!(
            r#"
tell application "{app_name}"
  if not (exists front window) then return ""
  if not (exists active tab of front window) then return ""
  set theURL to URL of active tab of front window
  set theTitle to title of active tab of front window
  set theText to execute active tab of front window javascript "(() => {{ try {{ return (document.body && document.body.innerText) ? document.body.innerText : document.documentElement.innerText; }} catch (e) {{ return ''; }} }})()"
  return theTitle & "\n" & theURL & "\n\n" & theText
end tell
"#
        )
    }

    // Only implement the "easy wins" that can reliably return full-page text.
    // Everything else falls back to screenshot/scroll.
    let script = match bundle_id {
        "com.apple.Safari" => r#"
tell application "Safari"
  if not (exists front document) then return ""
  set theURL to URL of front document
  set theTitle to name of front document
  set theText to do JavaScript "(() => { try { return (document.body && document.body.innerText) ? document.body.innerText : document.documentElement.innerText; } catch (e) { return ''; } })()" in front document
  return theTitle & "\n" & theURL & "\n\n" & theText
end tell
"#,
        // Chromium-family browsers.
        "com.google.Chrome" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Google Chrome"), "osascript:Google Chrome", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        "com.google.Chrome.canary" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Google Chrome Canary"), "osascript:Google Chrome Canary", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        "com.google.Chrome.beta" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Google Chrome Beta"), "osascript:Google Chrome Beta", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        "com.google.Chrome.dev" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Google Chrome Dev"), "osascript:Google Chrome Dev", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        "org.chromium.Chromium" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Chromium"), "osascript:Chromium", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        "com.microsoft.Edge" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Microsoft Edge"), "osascript:Microsoft Edge", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        "com.brave.Browser" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Brave Browser"), "osascript:Brave Browser", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        "company.thebrowser.Browser" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Arc"), "osascript:Arc", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        "com.vivaldi.Vivaldi" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Vivaldi"), "osascript:Vivaldi", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        "com.operasoftware.Opera" => {
            let ok = run_fullpage_script(snapshot, &chromium_script("Opera"), "osascript:Opera", &FULLPAGE_ERROR_LOGGED);
            if !ok {
                try_capture_full_page_text_via_accessibility(snapshot);
            }
            return;
        }
        _ => return,
    };

    let ok = run_fullpage_script(snapshot, script, "osascript:Safari", &FULLPAGE_ERROR_LOGGED);
    if !ok {
        try_capture_full_page_text_via_accessibility(snapshot);
    }
}

fn run_fullpage_script_raw(script: &str) -> Result<Option<String>, String> {
    let out = Command::new("osascript")
        .args(["-e", script])
        .output()
        .map_err(|e| format!("could not run osascript: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err("osascript returned non-zero exit".to_string());
        }
        return Err(stderr);
    }

    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Ok(None);
    }

    // Bound size to keep prompts manageable.
    let text = if text.len() > 16_000 {
        format!("{}…", truncate_utf8(&text, 16_000))
    } else {
        text
    };
    Ok(Some(text))
}

fn run_fullpage_script(
    snapshot: &mut ContextSnapshot,
    script: &str,
    method_label: &str,
    logged: &AtomicBool,
) -> bool {
    snapshot.full_page_method = Some(method_label.to_string());
    match run_fullpage_script_raw(script) {
        Ok(Some(text)) => {
            snapshot.full_page_text = Some(text);
            snapshot.full_page_error = None;
            true
        }
        Ok(None) => {
            snapshot.full_page_text = None;
            snapshot.full_page_error = Some("empty".to_string());
            false
        }
        Err(e) => {
            snapshot.full_page_text = None;
            snapshot.full_page_error = Some(e.clone());
            if !logged.swap(true, Ordering::Relaxed) {
                eprintln!("[interceptor] Full-page capture failed: {e}");
                eprintln!(
                    "[interceptor] Tip: grant Automation permission for Darling to control your browser, or set DARLING_CTX_FULLPAGE=0."
                );
            }
            false
        }
    }
}

fn is_known_browser_bundle_id(bundle_id: &str) -> bool {
    matches!(
        bundle_id,
        "com.apple.Safari"
            | "com.google.Chrome"
            | "com.google.Chrome.canary"
            | "com.google.Chrome.beta"
            | "com.google.Chrome.dev"
            | "org.chromium.Chromium"
            | "com.microsoft.Edge"
            | "com.brave.Browser"
            | "company.thebrowser.Browser"
            | "com.vivaldi.Vivaldi"
            | "com.operasoftware.Opera"
    )
}

fn try_capture_full_page_text_via_accessibility(snapshot: &mut ContextSnapshot) {
    if snapshot.full_page_text.is_some() {
        return;
    }
    let Some(bundle_id) = snapshot.bundle_id.as_deref() else {
        return;
    };
    if !is_known_browser_bundle_id(bundle_id) {
        return;
    }
    let Some(pid) = snapshot.pid else {
        return;
    };

    fn ax_copy_children(element: *mut c_void, attr: &str) -> Vec<*mut c_void> {
        let Some(arr) = ax_copy_attr_name(element, attr) else {
            return Vec::new();
        };
        let tid = unsafe { CFGetTypeID(arr as *const c_void) };
        if tid != unsafe { CFArrayGetTypeID() } {
            unsafe { CFRelease(arr) };
            return Vec::new();
        }

        let count = unsafe { CFArrayGetCount(arr as *const c_void) };
        let mut out: Vec<*mut c_void> = Vec::new();
        for idx in 0..count {
            let v = unsafe { CFArrayGetValueAtIndex(arr as *const c_void, idx) } as *mut c_void;
            if v.is_null() {
                continue;
            }
            let retained = unsafe { CFRetain(v as *const c_void) } as *mut c_void;
            if !retained.is_null() {
                out.push(retained);
            }
        }
        unsafe { CFRelease(arr) };
        out
    }

    fn ax_role(element: *mut c_void) -> Option<String> {
        let role = ax_copy_attr_name(element, "AXRole")?;
        let out = cf_type_to_string(role);
        unsafe { CFRelease(role) };
        out
    }

    fn ax_text_like(element: *mut c_void) -> Option<String> {
        for attr in ["AXValue", "AXDocument", "AXDescription"] {
            if let Some(v) = ax_copy_attr_name(element, attr) {
                let out = cf_type_to_string(v).map(|s| s.trim().to_string());
                unsafe { CFRelease(v) };
                if let Some(out) = out {
                    if !out.is_empty() {
                        return Some(out);
                    }
                }
            }
        }
        None
    }

    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return;
    }
    let win = ax_copy_attr_name(app, "AXFocusedWindow");
    unsafe { CFRelease(app) };
    let Some(win) = win else {
        return;
    };

    // BFS for a web/document node. Bound traversal to avoid pathological trees.
    let mut queue: Vec<*mut c_void> = vec![win];
    let mut visited = 0usize;
    let mut best_text: Option<String> = None;

    while let Some(node) = queue.pop() {
        visited += 1;
        if visited > 1200 {
            unsafe { CFRelease(node) };
            break;
        }

        if let Some(role) = ax_role(node) {
            if role == "AXWebArea" || role == "AXDocument" || role == "AXScrollArea" {
                if let Some(t) = ax_text_like(node) {
                    if t.len() > best_text.as_ref().map(|s| s.len()).unwrap_or(0) {
                        best_text = Some(t);
                    }
                }
            }
        }

        for attr in ["AXChildren", "AXContents"] {
            let kids = ax_copy_children(node, attr);
            for k in kids {
                queue.push(k);
            }
        }

        unsafe { CFRelease(node) };
    }

    let Some(mut text) = best_text else {
        if snapshot.full_page_error.is_none() {
            snapshot.full_page_error = Some("accessibility returned empty".to_string());
        }
        return;
    };
    if text.len() > 16_000 {
        text = format!("{}…", truncate_utf8(&text, 16_000));
    }
    snapshot.full_page_text = Some(text);
    snapshot.full_page_method = Some("accessibility".to_string());
    snapshot.full_page_error = None;
}

fn capture_screenshot_best_effort(snapshot: &mut ContextSnapshot) {
    static SCREENSHOT_ERROR_LOGGED: AtomicBool = AtomicBool::new(false);

    // Opt-in: Screen Recording permission may be required.
    if !env_flag("DARLING_CTX_SCREENSHOT", false) {
        return;
    }

    fn parse_u32_env(name: &str, default: u32) -> u32 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(default)
    }

    fn env_bool(name: &str) -> bool {
        matches!(
            std::env::var(name).unwrap_or_default().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    }

    fn scroll_pixels(delta: i32) {
        // Post a smooth scroll event to the session stream.
        unsafe {
            let ev = CGEventCreateScrollWheelEvent2(
                std::ptr::null_mut(),
                kCGScrollEventUnitPixel,
                1,
                delta,
                0,
                0,
            );
            if !ev.is_null() {
                CGEventPost(kCGSessionEventTap, ev);
                CFRelease(ev);
            }
        }
    }

    let mut scroll_capture = env_bool("DARLING_CTX_SCROLL_CAPTURE");
    let pages = parse_u32_env("DARLING_CTX_SCROLL_PAGES", 2);
    let pixels = parse_u32_env("DARLING_CTX_SCROLL_PIXELS", 900) as i32;
    let direction = std::env::var("DARLING_CTX_SCROLL_DIRECTION")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_else(|| {
            // Reasonable defaults:
            // - Chat apps (WeChat): usually need history above.
            // - Others: capture below by default.
            match snapshot.bundle_id.as_deref() {
                Some("com.tencent.xinWeChat") => "up".to_string(),
                _ => "down".to_string(),
            }
        });

    // Automatic safety: if the user is actively typing (text caret exists),
    // scrolling is intrusive and can change caret/selection.
    // Allow overriding with `DARLING_CTX_SCROLL_CAPTURE_WHILE_TYPING=1`.
    let allow_while_typing = env_bool("DARLING_CTX_SCROLL_CAPTURE_WHILE_TYPING");
    if snapshot.has_text_caret.unwrap_or(false) && !allow_while_typing {
        scroll_capture = false;
    }

    // Automatic safety: for known browsers we try full-page capture via scripts/AX.
    // If that fails, scrolling the viewport is disruptive; prefer a single screenshot.
    // Allow overriding with `DARLING_CTX_SCROLL_CAPTURE_IN_BROWSER=1`.
    let allow_in_browser = env_bool("DARLING_CTX_SCROLL_CAPTURE_IN_BROWSER");
    if !allow_in_browser {
        if let Some(bid) = snapshot.bundle_id.as_deref() {
            if is_known_browser_bundle_id(bid) {
                scroll_capture = false;
            }
        }
    }

    let pid = std::process::id();
    let mut paths: Vec<String> = Vec::new();
    let mut capture_idx = 0u32;

    let capture_once = |idx: u32| -> Option<String> {
        let mut pbuf = std::env::temp_dir();
        pbuf.push(format!("darling_ctx_{}_{}.png", pid, idx));
        let p = pbuf.to_string_lossy().to_string();
        let out = Command::new("screencapture")
            .args(["-x", "-t", "png", &p])
            .output();
        match out {
            Ok(out) if out.status.success() => Some(p),
            Ok(out) => {
                if !SCREENSHOT_ERROR_LOGGED.swap(true, Ordering::Relaxed) {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    if stderr.is_empty() {
                        eprintln!("[interceptor] Screenshot capture failed (non-zero exit).");
                    } else {
                        eprintln!("[interceptor] Screenshot capture failed: {stderr}");
                    }
                    eprintln!(
                        "[interceptor] Tip: enable Screen Recording permission for Darling, or set DARLING_CTX_SCREENSHOT=0."
                    );
                }
                None
            }
            Err(e) => {
                if !SCREENSHOT_ERROR_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!("[interceptor] Screenshot capture spawn failed: {e}");
                }
                None
            }
        }
    };

    if let Some(p) = capture_once(capture_idx) {
        snapshot.screenshot_path = Some(p.clone());
        paths.push(p);
        capture_idx += 1;
    } else {
        return;
    }

    if scroll_capture && pages > 0 {
        let want_down = direction == "down" || direction == "both";
        let want_up = direction == "up" || direction == "both";

        if want_down {
            for _ in 0..pages {
                scroll_pixels(-pixels);
                thread::sleep(Duration::from_millis(160));
                if let Some(p) = capture_once(capture_idx) {
                    paths.push(p);
                }
                capture_idx += 1;
            }
            // Restore to original position.
            for _ in 0..pages {
                scroll_pixels(pixels);
                thread::sleep(Duration::from_millis(80));
            }
        }

        if want_up {
            for _ in 0..pages {
                scroll_pixels(pixels);
                thread::sleep(Duration::from_millis(160));
                if let Some(p) = capture_once(capture_idx) {
                    paths.push(p);
                }
                capture_idx += 1;
            }
            // Restore to original position.
            for _ in 0..pages {
                scroll_pixels(-pixels);
                thread::sleep(Duration::from_millis(80));
            }
        }
    }

    if paths.len() > 1 {
        snapshot.screenshot_paths = Some(paths);
        snapshot.screenshot_capture_kind = Some("scroll".to_string());
        snapshot.screenshot_scroll_direction = Some(direction);
        snapshot.screenshot_scroll_pages = Some(pages);
        snapshot.screenshot_scroll_pixels = Some(pixels.unsigned_abs());
    } else if snapshot.screenshot_path.is_some() {
        snapshot.screenshot_capture_kind = Some("single".to_string());
    }
}

fn capture_frontmost_target_app() {
    let Some(frontmost) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
        return;
    };
    let Some(bundle_id) = frontmost.bundleIdentifier() else {
        return;
    };
    let bundle_id = bundle_id.to_string();

    // Avoid capturing ourselves; if the capsule was triggered while Darling was
    // frontmost, we don't need to refocus anything before pasting.
    if bundle_id == "com.aiagent.mac" {
        return;
    }

    let store = LAST_TARGET_BUNDLE_ID.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = store.lock() {
        *guard = Some(bundle_id);
    }
}

fn capture_context_snapshot() {
    let frontmost = NSWorkspace::sharedWorkspace().frontmostApplication();
    let mut snapshot = ContextSnapshot {
        bundle_id: None,
        app_name: None,
        pid: None,
        clipboard_text: None,
        window_title: None,
        selected_text: None,
        focused_role: None,
        has_text_caret: None,
        full_page_text: None,
        full_page_method: None,
        full_page_error: None,
        screenshot_path: None,
        screenshot_paths: None,
        screenshot_capture_kind: None,
        screenshot_scroll_direction: None,
        screenshot_scroll_pages: None,
        screenshot_scroll_pixels: None,
        ocr_text: None,
    };

    if let Some(app) = frontmost {
        if let Some(bundle_id) = app.bundleIdentifier() {
            let bid = bundle_id.to_string();
            if bid != "com.aiagent.mac" {
                snapshot.bundle_id = Some(bid);
            }
        }
        if let Some(name) = app.localizedName() {
            snapshot.app_name = Some(name.to_string());
        }
        snapshot.pid = Some(app.processIdentifier());
    }

    // Best-effort clipboard snapshot (helps if user copied the relevant message).
    // Truncate to avoid huge prompts.
    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(text) = cb.get_text() {
            let t = text.trim().to_string();
            if !t.is_empty() {
                // Avoid self-referential noise when the user copied our debug panel or logs.
                // (Common during development: clipboard contains JSON with keys like
                // `show_capsule_context` / `last_brain_debug`.)
                let looks_like_our_debug = t.starts_with('{')
                    && (t.contains("\"show_capsule_context\"")
                        || t.contains("\"last_brain_debug\"")
                        || t.contains("\"system_prompt_preview\""));
                if looks_like_our_debug {
                    // Skip clipboard entirely to keep prompts clean.
                    // The user can still explicitly ask us to use clipboard content.
                } else {
                let truncated = if t.len() > 2000 {
                    format!("{}…", truncate_utf8(&t, 2000))
                } else {
                    t
                };
                snapshot.clipboard_text = Some(truncated);
                }
            }
        }
    }

    capture_accessibility_context(&mut snapshot);

    // If we can capture full-page text (e.g., browser), do it before screenshots.
    try_capture_full_page_text(&mut snapshot);

    // Automatic strategy:
    // - If we successfully captured full-page text, skip screenshots by default
    //   (faster, less intrusive, and avoids sending images to the vision model).
    // - Allow opting back into screenshots even with full-page text.
    let force_screenshot = matches!(
        std::env::var("DARLING_CTX_SCREENSHOT_ALWAYS")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    );
    if snapshot.full_page_text.is_none() || force_screenshot {
        capture_screenshot_best_effort(&mut snapshot);
    }

    let store = LAST_CONTEXT_SNAPSHOT.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = store.lock() {
        *guard = Some(snapshot);
    }
}

fn activate_last_target_app() {
    let store = LAST_TARGET_BUNDLE_ID.get_or_init(|| Mutex::new(None));
    let bundle_id = match store.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    };
    let Some(bundle_id) = bundle_id else {
        return;
    };

    let Ok(cstr) = std::ffi::CString::new(bundle_id) else {
        return;
    };

    // SAFETY: CString is NUL-terminated; we keep it alive for the duration of the call.
    let ns_bundle = unsafe {
        NSString::initWithUTF8String(
            NSString::alloc(),
            std::ptr::NonNull::new(cstr.as_ptr() as *mut _).unwrap(),
        )
    };
    let Some(ns_bundle) = ns_bundle else {
        return;
    };

    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns_bundle);
    if apps.count() == 0 {
        return;
    }

    let target = apps.objectAtIndex(0);
    let _ = target.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);

    // Try to bring back the exact window that had focus when the capsule was triggered.
    // This reduces the chance of typing/paste landing in a different window/tab.
    raise_last_target_window();
}

extern "C" fn activate_last_target_on_main(_ctx: *mut c_void) {
    activate_last_target_app();
}

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
            // Record the app we are interrupting so we can paste back into it later.
            // Must happen before we activate our own app/window.
            capture_context_snapshot();
            capture_frontmost_target_app();

            let ns_ptr = match win.ns_window() {
                Ok(p) => p,
                Err(_) => return,
            };

            let ns_win: &NSWindow = unsafe {
                let ptr: *const NSWindow = ns_ptr as *const NSWindow;
                &*ptr
            };

            // Keep showing even when our app is not active.
            ns_win.setHidesOnDeactivate(false);

            // Try to explicitly attach to the currently active Space (best for
            // showing over other apps' fullscreen Spaces on macOS 15).
            // If that fails, fall back to AppKit behaviors.
            if !try_attach_window_to_active_space(ns_win) {
                // Note: `CanJoinAllSpaces` cannot be combined with `MoveToActiveSpace`.
                ns_win.orderOut(None);
                ns_win.setCollectionBehavior(
                    NSWindowCollectionBehavior::MoveToActiveSpace
                        | NSWindowCollectionBehavior::CanJoinAllApplications
                        | NSWindowCollectionBehavior::Transient
                        | NSWindowCollectionBehavior::FullScreenAuxiliary
                        | NSWindowCollectionBehavior::IgnoresCycle,
                );
            }

            // Maximum z-order so we render above the fullscreen app.
            ns_win.setLevel(MAX_WINDOW_LEVEL);

            // Show and take focus so the user can type immediately.
            ns_win.makeKeyAndOrderFront(None);
            ns_win.orderFrontRegardless();
            if let Some(mtm) = MainThreadMarker::new() {
                let app = NSApplication::sharedApplication(mtm);
                app.activateIgnoringOtherApps(true);
            }

            // Tell the frontend to mount the capsule *after* the window is visible/focused,
            // so the input autofocus is reliable.
            let payload = last_context_snapshot();
            let _ = handle.emit("show-capsule", payload);
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
            Ok(p) => inject_text_to_target(&p.text),
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
fn inject_text_to_target(text: &str) {
    let mode = std::env::var("DARLING_INJECT_MODE")
        .unwrap_or_else(|_| "unicode".to_string())
        .trim()
        .to_ascii_lowercase();

    match mode.as_str() {
        "clipboard" => inject_via_clipboard_paste(text, false),
        "clipboard_restore" | "clipboard-restore" => inject_via_clipboard_paste(text, true),
        "unicode" | "type" | "type_unicode" => inject_via_unicode(text),
        other => {
            eprintln!("[interceptor] Unknown DARLING_INJECT_MODE={other}; defaulting to unicode");
            inject_via_unicode(text)
        }
    }
}

fn inject_via_clipboard_paste(text: &str, restore: bool) {
    let previous = if restore {
        arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok())
    } else {
        None
    };

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

    // We brought Darling to the front to let the user type. For paste, we must
    // re-activate the previously frontmost app (e.g. VSCode fullscreen) so the
    // Cmd+V goes to the right window.
    unsafe {
        dispatch_async_f(
            std::ptr::addr_of!(_dispatch_main_q),
            std::ptr::null_mut(),
            activate_last_target_on_main,
        );
    }
    thread::sleep(Duration::from_millis(90));

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

    if restore {
        if let Some(prev) = previous {
            // Let the paste key event land before we restore.
            thread::sleep(Duration::from_millis(60));
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set_text(prev);
            }
        }
    }
}

fn inject_via_unicode(text: &str) {
    // Re-activate the previously frontmost app so the typed text lands in the right place.
    unsafe {
        dispatch_async_f(
            std::ptr::addr_of!(_dispatch_main_q),
            std::ptr::null_mut(),
            activate_last_target_on_main,
        );
    }
    thread::sleep(Duration::from_millis(90));

    // Post Unicode text as keyboard events (does not touch the clipboard).
    let utf16: Vec<u16> = text.encode_utf16().collect();
    if utf16.is_empty() {
        return;
    }

    // Chunk to avoid pathological large events.
    const CHUNK: usize = 6000;
    let mut i = 0;
    while i < utf16.len() {
        let end = (i + CHUNK).min(utf16.len());
        let chunk = &utf16[i..end];

        unsafe {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null_mut(), 0, true);
            if ev.is_null() {
                return;
            }
            CGEventKeyboardSetUnicodeString(ev, chunk.len(), chunk.as_ptr());
            CGEventPost(kCGSessionEventTap, ev);
            CFRelease(ev);
        }

        i = end;
        // Tiny delay so targets process long inserts smoothly.
        thread::sleep(Duration::from_millis(2));
    }
}
