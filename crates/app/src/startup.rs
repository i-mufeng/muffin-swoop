//! Current-user login startup. Reads OS state; mutations only follow an explicit Apply.
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", test))]
const LABEL: &str = "com.muffin.swoop";
/// 登录自启启动项附带的参数：程序以此识别“开机启动”，直接驻留托盘、不显示主窗口
pub const BACKGROUND_ARG: &str = "--background";
#[derive(Clone, Debug, Default)]
pub struct StartupState {
    pub enabled: bool,
    pub matches_current: bool,
    pub packaged: bool,
    pub detail: String,
}

enum Operation {
    Read,
    Set(bool),
}
pub struct StartupController {
    pub state: Option<StartupState>,
    pub desired: bool,
    pub error: Option<String>,
    pending: Option<mpsc::Receiver<Result<StartupState, String>>>,
    refreshed: Option<Instant>,
}
impl StartupController {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut controller = Self { state: None, desired: false, error: None, pending: None, refreshed: None };
        controller.start(Operation::Read, ctx);
        controller
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn refresh(&mut self, ctx: &egui::Context) {
        if !self.busy() {
            self.start(Operation::Read, ctx);
        }
    }
    pub fn refresh_if_stale(&mut self, ctx: &egui::Context) {
        if self.state.as_ref().is_some_and(|state| self.desired != state.enabled) {
            return;
        }
        if self.refreshed.is_none_or(|at| at.elapsed() > Duration::from_secs(5)) {
            self.refresh(ctx);
        }
    }
    pub fn apply(&mut self, ctx: &egui::Context) {
        if !self.busy() {
            self.start(Operation::Set(self.desired), ctx);
        }
    }
    fn start(&mut self, operation: Operation, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new().name("swoop-login-startup".into()).spawn(move || {
            let result = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
                if let Operation::Set(enabled) = operation {
                    if enabled && !packaged_path(&exe) {
                        return Err(swoop_i18n::tr!("请从完整安装包启动 Swoop 后配置开机启动").into());
                    }
                    platform::configure(&exe, enabled)?;
                }
                platform::read(&exe)
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        }) {
            Ok(_) => {
                self.pending = Some(rx);
                self.error = None;
            }
            Err(error) => self.error = Some(swoop_i18n::trf!("无法启动设置线程：{error}")),
        }
    }
    pub fn poll(&mut self) {
        let result = self.pending.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(swoop_i18n::tr!("启动项操作线程中断").into())),
        });
        if let Some(result) = result {
            self.pending = None;
            self.refreshed = Some(Instant::now());
            match result {
                Ok(state) => {
                    self.desired = state.enabled;
                    self.state = Some(state);
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
}

fn packaged_path(exe: &Path) -> bool {
    if !exe.is_file() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        let Some(macos) = exe.parent().filter(|p| p.file_name().is_some_and(|n| n == "MacOS")) else {
            return false;
        };
        let Some(contents) = macos.parent().filter(|p| p.file_name().is_some_and(|n| n == "Contents")) else {
            return false;
        };
        contents.join("Info.plist").is_file()
            && contents.parent().is_some_and(|p| p.extension().is_some_and(|e| e == "app"))
    }
    #[cfg(windows)]
    {
        exe.file_name().is_some_and(|n| n == "swoop.exe")
            && exe.parent().is_some_and(|p| p.join("swoop-cli.exe").is_file() && p.join("build-info.txt").is_file())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

/// 校验启动命令格式并取出路径范围：命令必须是 `"路径"`，之后只允许为空（旧格式）
/// 或后台参数。兼容旧版漏写分隔空格的命令，以便用户应用设置时修复。
#[cfg(any(windows, test))]
fn command_path_range(units: &[u16]) -> Result<std::ops::Range<usize>, String> {
    if units.len() < 2 || units.first() != Some(&34) {
        return Err(swoop_i18n::tr!("已有 Swoop 启动命令不是单一程序路径，未覆盖").into());
    }
    let Some(end) = units[1..].iter().position(|&unit| unit == 34).map(|position| position + 1) else {
        return Err(swoop_i18n::tr!("已有 Swoop 启动命令不是单一程序路径，未覆盖").into());
    };
    let tail = &units[end + 1..];
    let background: Vec<u16> = std::iter::once(0x20).chain(BACKGROUND_ARG.encode_utf16()).collect();
    if !tail.is_empty() && tail != background && tail != &background[1..] {
        return Err(swoop_i18n::tr!("已有 Swoop 启动命令包含无法识别的参数，未覆盖").into());
    }
    Ok(1..end)
}

/// REG_SZ 的完整 UTF-16 数据（含 NUL）。路径与参数之间必须有空格。
#[cfg(any(windows, test))]
fn background_command(path: impl IntoIterator<Item = u16>) -> Vec<u16> {
    std::iter::once(34).chain(path).chain([34, 32]).chain(BACKGROUND_ARG.encode_utf16()).chain([0]).collect()
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
enum Approval {
    Enabled,
    Disabled,
    Unknown,
}

#[cfg(any(windows, test))]
fn approval(bytes: Option<&[u8]>) -> Approval {
    // StartupApproved 是系统维护的非公开格式，只识别已知状态，不改写它。
    match bytes {
        None => Approval::Enabled,
        Some(bytes) if bytes.len() >= 12 && matches!(&bytes[..4], [2 | 6, 0, 0, 0]) => Approval::Enabled,
        Some(bytes) if bytes.len() >= 12 && matches!(&bytes[..4], [3 | 7, 0, 0, 0]) => Approval::Disabled,
        Some(_) => Approval::Unknown,
    }
}

fn state(exe: &Path, configured: Option<PathBuf>, disabled: bool) -> StartupState {
    let packaged = packaged_path(exe);
    match configured {
        None => StartupState { packaged, detail: swoop_i18n::tr!("未开启").into(), ..Default::default() },
        Some(path) => {
            let exists = path.is_file();
            let matches_current = path == exe;
            let detail = if disabled {
                swoop_i18n::tr!("系统已禁用，请在系统启动项中启用后刷新")
            } else if !exists {
                swoop_i18n::tr!("启动路径已失效，应用可重新配置")
            } else if !matches_current {
                swoop_i18n::tr!("已配置其他位置，应用可更新为当前位置")
            } else {
                swoop_i18n::tr!("已开启")
            }
            .into();
            StartupState { enabled: exists && !disabled, matches_current, packaged, detail }
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::process::Command;

    fn location() -> Result<PathBuf, String> {
        dirs::home_dir()
            .map(|home| home.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
            .ok_or(swoop_i18n::tr!("无法定位用户目录").into())
    }
    fn domain() -> Result<String, String> {
        let output = Command::new("/usr/bin/id").arg("-u").output().map_err(|_| swoop_i18n::tr!("无法读取当前用户"))?;
        let uid = String::from_utf8(output.stdout).map_err(|_| swoop_i18n::tr!("用户编号格式无效"))?;
        let uid = uid.trim().parse::<u32>().map_err(|_| swoop_i18n::tr!("用户编号格式无效"))?;
        Ok(format!("gui/{uid}"))
    }
    pub fn read(exe: &Path) -> Result<StartupState, String> {
        let configured = read_file(&location()?)?;
        if configured.is_none() {
            return Ok(state(exe, None, false));
        }
        let output = Command::new("/bin/launchctl")
            .args(["print-disabled", &domain()?])
            .output()
            .map_err(|_| swoop_i18n::tr!("无法读取系统启动项状态"))?;
        if !output.status.success() {
            return Err(swoop_i18n::tr!("系统未返回登录启动项状态").into());
        }
        let needle = format!("\"{LABEL}\" => true");
        let disabled =
            String::from_utf8_lossy(&output.stdout).lines().any(|line| line.trim().trim_end_matches(';') == needle);
        Ok(state(exe, configured, disabled))
    }
    pub fn configure(exe: &Path, enabled: bool) -> Result<(), String> {
        let path = location()?;
        // Validate ownership/schema before changing a file with this fixed label.
        let _ = read_file(&path)?;
        let target = format!("{}/{LABEL}", domain()?);
        let output = Command::new("/bin/launchctl")
            .args([if enabled { "enable" } else { "disable" }, &target])
            .output()
            .map_err(|_| swoop_i18n::tr!("无法更新系统启动项状态"))?;
        if !output.status.success() {
            return Err(swoop_i18n::tr!("系统拒绝更新启动项；未改写 LaunchAgent").into());
        }
        write_file(&path, exe, enabled)
            .map_err(|error| swoop_i18n::trf!("系统状态已更新，但保存启动项失败：{error}；请刷新后重试"))
    }
    pub(super) fn read_file(path: &Path) -> Result<Option<PathBuf>, String> {
        let meta = match path.symlink_metadata() {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 64 * 1024 {
            return Err(swoop_i18n::tr!("启动项不是有效的普通文件").into());
        }
        let output = Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(path)
            .output()
            .map_err(|_| swoop_i18n::tr!("无法读取 LaunchAgent"))?;
        if !output.status.success() {
            return Err(swoop_i18n::tr!("LaunchAgent 格式无效，未覆盖").into());
        }
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|_| swoop_i18n::tr!("LaunchAgent 格式无效"))?;
        if value["Label"] != LABEL {
            return Err(swoop_i18n::tr!("启动项标识不匹配，未覆盖").into());
        }
        let args = value["ProgramArguments"]
            .as_array()
            .filter(|args| args.len() == 1 || (args.len() == 2 && args[1] == BACKGROUND_ARG))
            .ok_or(swoop_i18n::tr!("启动参数不兼容，未覆盖"))?;
        let path = args[0].as_str().ok_or(swoop_i18n::tr!("启动路径无效"))?;
        if value["RunAtLoad"] != true || value["Disabled"] == true {
            return Ok(None);
        }
        Ok(Some(PathBuf::from(path)))
    }
    pub(super) fn write_file(path: &Path, exe: &Path, enabled: bool) -> Result<(), String> {
        let _ = read_file(path)?;
        if !enabled {
            return match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            };
        }
        let xml = launch_agent(exe)?;
        let parent = path.parent().ok_or(swoop_i18n::tr!("启动项目录无效"))?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temporary = parent.join(format!(".swoop-{}.tmp", uuid::Uuid::new_v4().simple()));
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
            file.write_all(xml.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)
        })();
        let _ = std::fs::remove_file(temporary);
        result.map_err(|e| e.to_string())
    }
}

#[cfg(any(target_os = "macos", test))]
fn launch_agent(exe: &Path) -> Result<String, String> {
    let path = exe
        .to_str()
        .filter(|s| !s.chars().any(|c| c.is_control()))
        .ok_or(swoop_i18n::tr!("启动路径包含不支持的字符"))?;
    let escaped = path
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array><string>{escaped}</string><string>{BACKGROUND_ARG}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><false/></dict></plist>\n"
    ))
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::*;
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVAL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn value(key: &str, flags: u32) -> Result<Option<Vec<u8>>, String> {
        let key = wide(key);
        let name = wide("Swoop");
        let mut size = 0;
        let first = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                flags,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if first == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if first != ERROR_SUCCESS || size > 64 * 1024 {
            return Err(swoop_i18n::trf!("读取启动项失败（{first}）"));
        }
        let mut data = vec![0u8; size as usize];
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                flags,
                std::ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if result != ERROR_SUCCESS {
            return Err(swoop_i18n::trf!("读取启动项失败（{result}）"));
        }
        data.truncate(size as usize);
        Ok(Some(data))
    }
    fn current(key: &str) -> Result<Option<(PathBuf, bool)>, String> {
        let Some(bytes) = value(key, RRF_RT_REG_SZ)? else {
            return Ok(None);
        };
        if bytes.len() % 2 != 0 {
            return Err(swoop_i18n::tr!("启动路径编码无效").into());
        }
        let mut units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
        if units.last() == Some(&0) {
            units.pop();
        }
        let range = command_path_range(&units)?;
        let background = units.get(range.end + 1) == Some(&32);
        Ok(Some((PathBuf::from(std::ffi::OsString::from_wide(&units[range])), background)))
    }
    pub fn read(exe: &Path) -> Result<StartupState, String> {
        read_at(exe, KEY, APPROVAL_KEY)
    }
    fn read_at(exe: &Path, key: &str, approval_key: &str) -> Result<StartupState, String> {
        let Some((configured, background)) = current(key)? else {
            return Ok(state(exe, None, false));
        };
        let approval = approval(value(approval_key, RRF_RT_REG_BINARY)?.as_deref());
        let mut state = state(exe, Some(configured), approval == Approval::Disabled);
        if approval == Approval::Unknown && state.enabled {
            // 注册成功与系统是否允许启动是两件事；未知格式不应让配置本身失败。
            state.detail = swoop_i18n::tr!("启动项已登记，请到 Windows 设置确认是否启用").into();
        } else if state.enabled && !background {
            state.detail = swoop_i18n::tr!("启动项需更新，点击应用后将在后台启动").into();
        }
        state.matches_current &= background;
        Ok(state)
    }
    pub fn configure(exe: &Path, enabled: bool) -> Result<(), String> {
        configure_at(exe, enabled, KEY, APPROVAL_KEY)
    }
    fn configure_at(exe: &Path, enabled: bool, key: &str, approval_key: &str) -> Result<(), String> {
        let _ = current(key)?;
        let key_name = key;
        let key = wide(key);
        let name = wide("Swoop");
        let mut handle = std::ptr::null_mut();
        let open = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut handle,
                std::ptr::null_mut(),
            )
        };
        if open != ERROR_SUCCESS {
            return Err(swoop_i18n::trf!("无法打开当前用户启动项（{open}）"));
        }
        let result = if enabled {
            let command = background_command(exe.as_os_str().encode_wide());
            unsafe {
                RegSetValueExW(handle, name.as_ptr(), 0, REG_SZ, command.as_ptr().cast(), (command.len() * 2) as u32)
            }
        } else {
            unsafe { RegDeleteValueW(handle, name.as_ptr()) }
        };
        unsafe {
            RegCloseKey(handle);
        }
        if result != ERROR_SUCCESS && !(result == ERROR_FILE_NOT_FOUND && !enabled) {
            return Err(swoop_i18n::trf!("保存启动项失败（{result}）"));
        }
        if enabled && !read_at(exe, key_name, approval_key)?.enabled {
            return Err(
                swoop_i18n::tr!("启动路径已登记，但系统仍禁用此启动项；请在 Windows 启动应用中启用并刷新").into()
            );
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        struct RegistryFixture(String);
        impl Drop for RegistryFixture {
            fn drop(&mut self) {
                unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(&self.0).as_ptr()) };
            }
        }
        fn put(key: &str, kind: u32, bytes: &[u8]) {
            let mut handle = std::ptr::null_mut();
            assert_eq!(
                unsafe {
                    RegCreateKeyExW(
                        HKEY_CURRENT_USER,
                        wide(key).as_ptr(),
                        0,
                        std::ptr::null(),
                        REG_OPTION_NON_VOLATILE,
                        KEY_SET_VALUE,
                        std::ptr::null(),
                        &mut handle,
                        std::ptr::null_mut(),
                    )
                },
                ERROR_SUCCESS
            );
            let result =
                unsafe { RegSetValueExW(handle, wide("Swoop").as_ptr(), 0, kind, bytes.as_ptr(), bytes.len() as u32) };
            unsafe { RegCloseKey(handle) };
            assert_eq!(result, ERROR_SUCCESS);
        }
        #[test]
        fn registry_enable_repair_unknown_approval_and_disable() {
            // 使用独立临时键，不改写真实登录启动配置。
            let fixture = RegistryFixture(format!(r"Software\Swoop\StartupTests\{}", uuid::Uuid::new_v4()));
            let run = format!(r"{}\Run", fixture.0);
            let approved = format!(r"{}\Approved", fixture.0);
            let exe = std::env::current_exe().unwrap();
            assert!(!read_at(&exe, &run, &approved).unwrap().enabled);
            configure_at(&exe, true, &run, &approved).unwrap();
            let state = read_at(&exe, &run, &approved).unwrap();
            assert!(state.enabled && state.matches_current);

            // 复现旧版本少空格的命令，并通过重新应用修复。
            let mut broken = background_command(exe.as_os_str().encode_wide());
            let space = broken.len() - BACKGROUND_ARG.encode_utf16().count() - 2;
            assert_eq!(broken.remove(space), 32);
            let bytes: Vec<_> = broken.iter().flat_map(|unit| unit.to_le_bytes()).collect();
            put(&run, REG_SZ, &bytes);
            assert!(!read_at(&exe, &run, &approved).unwrap().matches_current);
            configure_at(&exe, true, &run, &approved).unwrap();
            assert!(read_at(&exe, &run, &approved).unwrap().matches_current);

            put(&approved, REG_BINARY, &[0; 12]);
            configure_at(&exe, true, &run, &approved).unwrap();
            let state = read_at(&exe, &run, &approved).unwrap();
            assert!(state.detail.contains("Windows"));
            assert_eq!(value(&approved, RRF_RT_REG_BINARY).unwrap(), Some(vec![0; 12]));
            let mut disabled = [0; 12];
            disabled[0] = 3;
            put(&approved, REG_BINARY, &disabled);
            assert!(!read_at(&exe, &run, &approved).unwrap().enabled);
            assert!(configure_at(&exe, true, &run, &approved).is_err());
            configure_at(&exe, false, &run, &approved).unwrap();
            assert!(current(&run).unwrap().is_none());
            assert_eq!(value(&approved, RRF_RT_REG_BINARY).unwrap(), Some(disabled.to_vec()));
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::*;
    pub fn read(_: &Path) -> Result<StartupState, String> {
        Err(swoop_i18n::tr!("当前平台暂不支持开机启动").into())
    }
    pub fn configure(_: &Path, _: bool) -> Result<(), String> {
        Err(swoop_i18n::tr!("当前平台暂不支持开机启动").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }
    #[test]
    fn startup_command_accepts_plain_path_and_background_flag_only() {
        let plain = wide("\"C:\\Apps\\swoop.exe\"");
        assert_eq!(command_path_range(&plain).unwrap(), 1..plain.len() - 1);

        let with_flag = wide("\"C:\\Apps\\swoop.exe\" --background");
        assert_eq!(command_path_range(&with_flag).unwrap(), 1..plain.len() - 1);

        // 旧版本注册的纯路径和当前版本带参数的命令都合法；其余格式一律拒绝
        assert!(command_path_range(&wide("C:\\Apps\\swoop.exe")).is_err());
        assert!(command_path_range(&wide("\"C:\\Apps\\swoop.exe\" --tray")).is_err());
        assert!(command_path_range(&wide("\"C:\\Apps\\swoop.exe\" --background extra")).is_err());
        assert!(command_path_range(&wide("\"unterminated")).is_err());
        assert!(command_path_range(&wide("\"C:\\Apps\\swoop.exe\"--background")).is_ok());
    }
    #[test]
    fn windows_background_command_roundtrips_unicode_path_and_separates_flag() {
        let path = wide(r"C:\应用 程序\swoop.exe");
        let command = background_command(path.iter().copied());
        assert_eq!(command.last(), Some(&0));
        let command = &command[..command.len() - 1];
        let range = command_path_range(command).unwrap();
        assert_eq!(&command[range.clone()], path);
        assert_eq!(&command[range.end + 1..], wide(" --background"));
    }
    #[test]
    fn windows_unknown_approval_is_distinct_from_disabled() {
        assert_eq!(approval(None), Approval::Enabled);
        for (byte, expected) in [
            (2, Approval::Enabled),
            (6, Approval::Enabled),
            (3, Approval::Disabled),
            (7, Approval::Disabled),
            (0, Approval::Unknown),
            (9, Approval::Unknown),
        ] {
            let mut bytes = [0; 12];
            bytes[0] = byte;
            assert_eq!(approval(Some(&bytes)), expected);
        }
        assert_eq!(approval(Some(&[2])), Approval::Unknown);
        assert_eq!(approval(Some(&[3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])), Approval::Unknown);
    }
    #[test]
    fn serialization_preserves_unicode_spaces_and_xml_specials() {
        let path = Path::new("/Applications/中文 & <test> 'a' \"b\".app/Contents/MacOS/swoop");
        let xml = launch_agent(path).unwrap();
        assert!(xml.contains(BACKGROUND_ARG));
        assert!(xml.contains("中文 &amp; &lt;test&gt; &apos;a&apos; &quot;b&quot;"));
        assert!(launch_agent(Path::new("bad\npath")).is_err());
        assert!(!packaged_path(Path::new("/missing/swoop")));
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn fixture_enable_disable_relocation_and_invalid_file_failure() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("中文 & <test> 'a' \"b\".app/Contents/MacOS/swoop");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"fixture").unwrap();
        std::fs::write(exe.parent().unwrap().parent().unwrap().join("Info.plist"), b"fixture").unwrap();
        assert!(packaged_path(&exe));
        let agent = dir.path().join("LaunchAgents/test.plist");
        assert!(platform::read_file(&agent).unwrap().is_none());
        platform::write_file(&agent, &exe, true).unwrap();
        assert!(std::fs::read_to_string(&agent).unwrap().contains(BACKGROUND_ARG));
        assert_eq!(platform::read_file(&agent).unwrap(), Some(exe.clone()));
        assert!(state(&exe, Some(exe.clone()), false).enabled);
        std::fs::rename(&exe, exe.with_file_name("moved")).unwrap();
        assert!(!state(&exe, Some(exe.clone()), false).enabled);
        platform::write_file(&agent, &exe, false).unwrap();
        assert!(!agent.exists());
        std::fs::write(&agent, "invalid").unwrap();
        assert!(platform::write_file(&agent, &exe, true).is_err());
        assert_eq!(std::fs::read_to_string(&agent).unwrap(), "invalid");
        let blocked = dir.path().join("not-directory");
        std::fs::write(&blocked, "file").unwrap();
        assert!(platform::write_file(&blocked.join("agent.plist"), &exe, true).is_err());
    }
}
