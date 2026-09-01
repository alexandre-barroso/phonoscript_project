//! Minimal AppKit bridge for Finder and Launch Services `.ottab` open events.

use std::path::PathBuf;
use std::sync::Mutex;

use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::sel;
use objc2_foundation::{NSArray, NSURL};

static OPENED_FILES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

unsafe extern "C" fn application_open_urls(
    _delegate: *mut AnyObject,
    _selector: Sel,
    _application: *mut AnyObject,
    urls: *mut NSArray<NSURL>,
) {
    let Some(urls) = (unsafe { urls.as_ref() }) else {
        return;
    };
    for url in urls {
        if unsafe { url.isFileURL() }
            && let Some(path) = unsafe { url.path() }
            && let Ok(mut queue) = OPENED_FILES.lock()
        {
            queue.push(PathBuf::from(path.to_string()));
        }
    }
}

pub fn install_file_open_handler() {
    // Winit owns the NSApplication delegate and requires it to remain in place.
    // AppKit discovers optional delegate selectors dynamically, so adding the
    // Open Document selector to that class preserves Winit's lifecycle while
    // allowing Launch Services to deliver `.ottab` URLs.
    let class = AnyClass::get("WinitApplicationDelegate")
        .expect("Winit application delegate class must already be registered");
    let selector = sel!(application:openURLs:);
    if class.instance_method(selector).is_some() {
        return;
    }
    let implementation = application_open_urls
        as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>);
    let erased: unsafe extern "C" fn() = unsafe { std::mem::transmute(implementation) };
    let class_pointer: *mut objc2::ffi::objc_class = (class as *const AnyClass).cast_mut().cast();
    let added = unsafe {
        objc2::ffi::class_addMethod(
            class_pointer,
            selector.as_ptr(),
            Some(erased),
            c"v@:@@".as_ptr(),
        )
    };
    assert!(added, "could not install the macOS Open Document handler");
}

pub fn take_opened_files() -> Vec<PathBuf> {
    match OPENED_FILES.lock() {
        Ok(mut queue) => std::mem::take(&mut *queue),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    }
}
