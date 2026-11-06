//! Work for the main thread from any thread: `on_main(f)` queues `f` on the main dispatch queue
//! (`dispatch_async_f`), which AppKit drains in every run-loop mode, menu tracking and live
//! resizing included. The engine's alert notify, workers (RSS, search, updates, URL fetches) and
//! anything else off the main thread wake the GUI through it (docs/MACOS_GUI.md "Threading and
//! state"). A few lines of libdispatch FFI rather than the `dispatch2` crate.

use std::ffi::c_void;

// libdispatch is part of libSystem, which every program links.
unsafe extern "C" {
    /// The main queue's object; `dispatch_get_main_queue()` is a macro returning its address.
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

type Job = Box<dyn FnOnce() + Send>;

extern "C" fn trampoline(context: *mut c_void) {
    // SAFETY: `context` is the `Box<Job>` that `on_main` leaked for exactly this one call.
    let job = unsafe { Box::from_raw(context.cast::<Job>()) };
    // a panic must not unwind into libdispatch (undefined behaviour): it ends the process
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
        std::process::abort();
    }
}

/// Runs `f` on the main thread soon, after whatever the main thread is doing now (never inside
/// the caller, even on the main thread: AppKit calls back synchronously, so this is also the
/// way to leave a borrow of the app's state before running something that needs it).
pub fn on_main(f: impl FnOnce() + Send + 'static) {
    let job: Box<Job> = Box::new(Box::new(f));
    // SAFETY: the main queue lives for the whole process; the context is a leaked box that
    // `trampoline` takes back exactly once.
    unsafe {
        dispatch_async_f(
            std::ptr::addr_of!(_dispatch_main_q),
            Box::into_raw(job).cast::<c_void>(),
            trampoline,
        )
    };
}
