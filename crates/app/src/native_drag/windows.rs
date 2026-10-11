use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::{DropPayload, Shared, drop_urls};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, POINTL};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, TYMED_HGLOBAL};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    OleInitialize, OleUninitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::core::{Ref, implement, w};

#[implement(IDropTarget)]
struct Target {
    shared: Arc<Shared>,
    accepted: AtomicBool,
}
#[allow(non_snake_case)]
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        object: Ref<'_, IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        _: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let accepted = self.shared.enabled.load(Ordering::Acquire) && object.as_ref().and_then(read_payload).is_some();
        self.accepted.store(accepted, Ordering::Release);
        unsafe {
            if let Some(effect) = effect.as_mut() {
                *effect = if accepted { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
            }
        }
        Ok(())
    }
    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let accepted = self.shared.enabled.load(Ordering::Acquire) && self.accepted.load(Ordering::Acquire);
        unsafe {
            if let Some(effect) = effect.as_mut() {
                *effect = if accepted { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
            }
        }
        Ok(())
    }
    fn DragLeave(&self) -> windows::core::Result<()> {
        self.accepted.store(false, Ordering::Release);
        Ok(())
    }
    fn Drop(
        &self,
        object: Ref<'_, IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        _: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let payload =
            if self.shared.enabled.load(Ordering::Acquire) { object.as_ref().and_then(read_payload) } else { None };
        let accepted = payload.is_some();
        if let Some(payload) = payload {
            self.shared.push(payload);
        }
        unsafe {
            if let Some(effect) = effect.as_mut() {
                *effect = if accepted { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
            }
        }
        self.accepted.store(false, Ordering::Release);
        Ok(())
    }
}
struct Medium(STGMEDIUM);
impl Drop for Medium {
    fn drop(&mut self) {
        unsafe {
            ReleaseStgMedium(&mut self.0);
        }
    }
}
fn medium(object: &IDataObject, format: u16) -> Option<Medium> {
    let format = FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let data = Medium(unsafe { object.GetData(&format).ok()? });
    if data.0.tymed == TYMED_HGLOBAL.0 as u32 { Some(data) } else { None }
}
fn read_payload(object: &IDataObject) -> Option<DropPayload> {
    for (format, wide) in [
        (unsafe { RegisterClipboardFormatW(w!("UniformResourceLocatorW")) } as u16, true),
        (unsafe { RegisterClipboardFormatW(w!("UniformResourceLocator")) } as u16, false),
        (CF_UNICODETEXT.0, true),
    ] {
        let Some(data) = medium(object, format) else {
            continue;
        };
        let handle = unsafe { data.0.u.hGlobal };
        let size = unsafe { GlobalSize(handle) };
        if size == 0 || size > crate::link_import::MAX_IMPORT_BYTES {
            continue;
        }
        let pointer = unsafe { GlobalLock(handle) };
        if pointer.is_null() {
            continue;
        }
        let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size) };
        let text = if wide {
            let units = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
                .take_while(|unit| *unit != 0)
                .collect::<Vec<_>>();
            String::from_utf16(&units).ok()
        } else {
            std::str::from_utf8(bytes.split(|byte| *byte == 0).next().unwrap_or_default()).ok().map(str::to_owned)
        };
        unsafe {
            let _ = GlobalUnlock(handle);
        }
        if let Some(urls) = text.and_then(|text| drop_urls(&text)) {
            return Some(DropPayload::Urls(urls));
        }
    }
    let data = medium(object, CF_HDROP.0)?;
    let handle = unsafe { HDROP(data.0.u.hGlobal.0) };
    let count = unsafe { DragQueryFileW(handle, u32::MAX, None) };
    if count == 0 || count > 1000 {
        return None;
    }
    let mut files = Vec::new();
    for index in 0..count {
        let length = unsafe { DragQueryFileW(handle, index, None) };
        if length == 0 || length > 32768 {
            return None;
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = unsafe { DragQueryFileW(handle, index, Some(&mut buffer)) };
        if copied != length {
            return None;
        }
        files.push(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize])));
    }
    Some(DropPayload::Files(files))
}
pub(super) struct Guard {
    window: HWND,
    _target: IDropTarget,
}
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.window);
            OleUninitialize();
        }
    }
}
pub(super) fn install(frame: &eframe::Frame, shared: Arc<Shared>) -> Result<Guard, String> {
    let RawWindowHandle::Win32(handle) = frame.window_handle().map_err(|error| error.to_string())?.as_raw() else {
        return Err(swoop_i18n::tr!("窗口不是 Win32").into());
    };
    let window = HWND(handle.hwnd.get() as *mut _);
    unsafe {
        OleInitialize(None).map_err(|error| error.to_string())?;
    }
    let target: IDropTarget = Target { shared, accepted: AtomicBool::new(false) }.into();
    if let Err(error) = unsafe { RegisterDragDrop(window, &target) } {
        unsafe {
            OleUninitialize();
        }
        return Err(error.to_string());
    }
    Ok(Guard { window, _target: target })
}
