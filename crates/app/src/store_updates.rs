//! Store 只读检查：不请求下载/安装，不使用站外更新源。
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use windows::ApplicationModel::Package;
use windows::Services::Store::StoreContext;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::Shell::IInitializeWithWindow;
use windows::core::Interface;
use windows_future::AsyncStatus;

pub fn open() -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::w;
    // egui 的 URL 打开器会先启动默认浏览器；这里直接交给系统的商店协议处理器。
    let result = unsafe {
        ShellExecuteW(
            Some(HWND(crate::tray::window_handle() as *mut _)),
            w!("open"),
            w!("ms-windows-store://pdp/?ProductId=9P9FNXJXK17N"),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code <= 32 { Err(swoop_i18n::trf!("无法打开 Microsoft Store（错误 {code}）")) } else { Ok(()) }
}

pub fn check(window: isize, cancel: &CancellationToken) -> Result<bool, String> {
    // 在专用工作线程初始化 WinRT，不阻塞 UI，也不在应用启动时加载商店服务。
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string())?;
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { RoUninitialize() };
        }
    }
    let _apartment = Apartment;
    let result = || -> windows::core::Result<_> {
        let family = Package::Current()?.Id()?.FamilyName()?;
        let context = StoreContext::GetDefault()?;
        let initialize: IInitializeWithWindow = context.cast()?;
        unsafe { initialize.Initialize(HWND(window as *mut _))? };
        let operation = context.GetAppAndOptionalStorePackageUpdatesAsync()?;
        Ok((family, operation))
    };
    let (family, operation) = result().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while operation.Status().map_err(|e| e.to_string())? == AsyncStatus::Started {
        if cancel.is_cancelled() || Instant::now() >= deadline {
            let _ = operation.Cancel();
            return Err(if cancel.is_cancelled() {
                swoop_i18n::tr!("更新已取消")
            } else {
                swoop_i18n::tr!("商店检查超时，请打开 Microsoft Store 查看")
            }
            .into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let updates = operation.GetResults().map_err(|e| e.to_string())?;
    for update in updates {
        let update_family =
            update.Package().and_then(|p| p.Id()).and_then(|id| id.FamilyName()).map_err(|e| e.to_string())?;
        if update_family == family {
            return Ok(true);
        }
    }
    Ok(false)
}
