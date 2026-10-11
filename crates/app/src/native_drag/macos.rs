use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex, OnceLock, Weak, atomic::Ordering};

use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel},
    sel,
};
use objc2_foundation::{NSArray, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{DropPayload, Shared, drop_urls};

fn bindings() -> &'static Mutex<HashMap<usize, Weak<Shared>>> {
    static BINDINGS: OnceLock<Mutex<HashMap<usize, Weak<Shared>>>> = OnceLock::new();
    BINDINGS.get_or_init(Default::default)
}
pub(super) struct Guard {
    delegate: Retained<AnyObject>,
    original: &'static AnyClass,
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut map) = bindings().lock() {
            map.remove(&(&*self.delegate as *const AnyObject as usize));
        }
        // This object is retained for the lifetime of the hook. No ivars were added.
        unsafe {
            AnyObject::set_class(&self.delegate, self.original);
        }
    }
}
unsafe fn read_urls(sender: &AnyObject) -> Option<String> {
    let pasteboard: Option<Retained<AnyObject>> = unsafe { msg_send![sender, draggingPasteboard] };
    let pasteboard = pasteboard?;
    unsafe { urls_from_pasteboard(&pasteboard) }
}
unsafe fn urls_from_pasteboard(pasteboard: &AnyObject) -> Option<String> {
    for kind in ["public.url", "public.utf8-plain-text", "NSStringPboardType"] {
        let text: Option<Retained<NSString>> =
            unsafe { msg_send![pasteboard, stringForType: &*NSString::from_str(kind)] };
        if let Some(text) = text
            && let Some(urls) = drop_urls(&text.to_string())
        {
            return Some(urls);
        }
    }
    None
}
fn shared(object: &AnyObject) -> Option<Arc<Shared>> {
    bindings().lock().ok()?.get(&(object as *const AnyObject as usize))?.upgrade()
}
unsafe fn original(object: &AnyObject, selector: Sel, sender: &AnyObject) -> Bool {
    let Some(method) = object.class().superclass().and_then(|class| class.instance_method(selector)) else {
        return Bool::NO;
    };
    // The original winit delegate methods use Objective-C BOOL for these selectors.
    let function: unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject) -> Bool =
        unsafe { std::mem::transmute(method.implementation()) };
    unsafe { function(object, selector, sender) }
}
unsafe extern "C-unwind" fn entered(object: &AnyObject, selector: Sel, sender: &AnyObject) -> Bool {
    if let Some(shared) = shared(object) {
        if !shared.enabled.load(Ordering::Acquire) {
            return Bool::NO;
        }
        if unsafe { read_urls(sender) }.is_some() {
            return Bool::YES;
        }
    }
    unsafe { original(object, selector, sender) }
}
unsafe extern "C-unwind" fn dropped(object: &AnyObject, selector: Sel, sender: &AnyObject) -> Bool {
    if let Some(shared) = shared(object) {
        if !shared.enabled.load(Ordering::Acquire) {
            return Bool::NO;
        }
        if let Some(urls) = unsafe { read_urls(sender) } {
            shared.push(DropPayload::Urls(urls));
            return Bool::YES;
        }
    }
    unsafe { original(object, selector, sender) }
}
pub(super) fn install(frame: &eframe::Frame, shared: Arc<Shared>) -> Result<Guard, String> {
    let RawWindowHandle::AppKit(handle) = frame.window_handle().map_err(|error| error.to_string())?.as_raw() else {
        return Err(swoop_i18n::tr!("窗口不是 AppKit").into());
    };
    // Called only on eframe's main UI thread. Retain the existing window delegate,
    // and subclass this instance rather than replacing winit's window callbacks.
    unsafe {
        let view = &*(handle.ns_view.as_ptr() as *const AnyObject);
        let window: Retained<AnyObject> = msg_send![view, window];
        let delegate: Retained<AnyObject> = msg_send![&*window, delegate];
        let original = delegate.class();
        let name = CString::new(format!("SwoopURLDrop_{:p}", original)).map_err(|error| error.to_string())?;
        let subclass = if let Some(class) = AnyClass::get(&name) {
            class
        } else {
            let mut builder = ClassBuilder::new(&name, original).ok_or(swoop_i18n::tr!("无法创建拖入代理类"))?;
            builder.add_method(sel!(draggingEntered:), entered as unsafe extern "C-unwind" fn(_, _, _) -> _);
            builder.add_method(sel!(performDragOperation:), dropped as unsafe extern "C-unwind" fn(_, _, _) -> _);
            builder.register()
        };
        bindings()
            .lock()
            .map_err(|_| swoop_i18n::tr!("拖入状态锁不可用"))?
            .insert(&*delegate as *const AnyObject as usize, Arc::downgrade(&shared));
        AnyObject::set_class(&delegate, subclass);
        let types = NSArray::from_retained_slice(&[
            NSString::from_str("NSFilenamesPboardType"),
            NSString::from_str("public.url"),
            NSString::from_str("public.utf8-plain-text"),
            NSString::from_str("NSStringPboardType"),
        ]);
        let _: () = msg_send![&*window, registerForDraggedTypes: &*types];
        Ok(Guard { delegate, original })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_pasteboard_url_and_plain_text_formats_are_read_without_global_clipboard() {
        use objc2_app_kit::NSPasteboard;
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        pasteboard.clearContents();
        assert!(pasteboard.setString_forType(
            &NSString::from_str("https://example.test/from-browser"),
            &NSString::from_str("public.url")
        ));
        assert_eq!(unsafe { urls_from_pasteboard(&pasteboard) }, Some("https://example.test/from-browser".into()));
        pasteboard.clearContents();
        assert!(pasteboard.setString_forType(
            &NSString::from_str("https://example.test/one\nhttps://example.test/two"),
            &NSString::from_str("public.utf8-plain-text")
        ));
        assert_eq!(
            unsafe { urls_from_pasteboard(&pasteboard) },
            Some("https://example.test/one\nhttps://example.test/two".into())
        );
        pasteboard.clearContents();
        assert!(
            pasteboard
                .setString_forType(&NSString::from_str("file:///private/user-file"), &NSString::from_str("public.url"))
        );
        assert!(unsafe { urls_from_pasteboard(&pasteboard) }.is_none());
        pasteboard.clearContents();
    }
}
