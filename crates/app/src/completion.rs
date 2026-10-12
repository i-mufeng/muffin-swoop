//! 下载后动作。配置默认关闭；外部程序在有界工作线程执行，不经过 shell。
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};
use swoop_core::{Store, TaskId};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileAction {
    #[default]
    None,
    Reveal,
    Open,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdleAction {
    #[default]
    None,
    Quit,
    Shutdown,
}
impl IdleAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => swoop_i18n::tr!("不执行"),
            Self::Quit => swoop_i18n::tr!("退出 Swoop"),
            Self::Shutdown => swoop_i18n::tr!("关闭电脑"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scanner {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub timeout_secs: u64,
}
impl Default for Scanner {
    fn default() -> Self {
        Self { program: PathBuf::new(), arguments: vec!["{file}".into()], timeout_secs: 300 }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompletionSettings {
    pub file_action: FileAction,
    pub scanner: Option<Scanner>,
    pub show_dialog: bool,
    pub idle_action: IdleAction,
    pub countdown_secs: u64,
    /// 新建下载时"安装包运行后移到回收站"的勾选状态，下次打开对话框沿用
    pub auto_install: bool,
}
impl Default for CompletionSettings {
    fn default() -> Self {
        Self {
            file_action: FileAction::None,
            scanner: None,
            show_dialog: false,
            idle_action: IdleAction::None,
            countdown_secs: 30,
            auto_install: false,
        }
    }
}
impl CompletionSettings {
    pub fn load(store: &Store) -> Self {
        store
            .get_setting("completion_actions")
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str::<Self>(&v).ok())
            .filter(|v| v.validate(false).is_ok())
            .unwrap_or_default()
    }
    fn validate(&self, check_program: bool) -> Result<(), String> {
        if !(10..=3600).contains(&self.countdown_secs) {
            return Err(swoop_i18n::tr!("倒计时应为 10–3600 秒").into());
        }
        if let Some(scanner) = &self.scanner {
            if !scanner.program.is_absolute() || (check_program && !scanner.program.is_file()) {
                return Err(swoop_i18n::tr!("请选择扫描程序的绝对路径").into());
            }
            if !(1..=86400).contains(&scanner.timeout_secs) {
                return Err(swoop_i18n::tr!("扫描超时应为 1–86400 秒").into());
            }
            if scanner.arguments.len() > 64 || scanner.arguments.iter().any(|s| s.len() > 8192 || s.contains('\0')) {
                return Err(swoop_i18n::tr!("扫描参数过长或包含无效字符").into());
            }
            if !scanner.arguments.iter().any(|s| s.contains("{file}")) {
                return Err(swoop_i18n::tr!("扫描参数至少包含一个 {file}").into());
            }
        }
        Ok(())
    }
    /// 新建下载对话框里切换勾选：只改这一项，不重新校验扫描程序是否存在
    pub fn set_auto_install(&mut self, store: &Store, value: bool) -> Result<(), String> {
        self.auto_install = value;
        store
            .set_setting("completion_actions", &serde_json::to_string(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        self.validate(true)?;
        store
            .set_setting("completion_actions", &serde_json::to_string(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
}

/// 仅新完成事件能武装倒计时；启动时已有的完成历史不能触发退出或关机。
#[derive(Default)]
pub struct Countdown {
    armed: bool,
    deadline: Option<Instant>,
}
impl Countdown {
    pub fn completed(&mut self) {
        self.armed = true;
    }
    pub fn cancel(&mut self) {
        self.armed = false;
        self.deadline = None;
    }
    pub fn remaining(&self, now: Instant) -> Option<u64> {
        self.deadline.map(|d| d.saturating_duration_since(now).as_millis().div_ceil(1000) as u64)
    }
    pub fn tick(&mut self, now: Instant, ready: bool, settings: &CompletionSettings) -> Option<IdleAction> {
        if settings.idle_action == IdleAction::None {
            self.cancel();
            return None;
        }
        if !ready {
            self.deadline = None;
            return None;
        }
        if !self.armed {
            return None;
        }
        let deadline = self.deadline.get_or_insert_with(|| now + Duration::from_secs(settings.countdown_secs));
        if now >= *deadline {
            self.cancel();
            Some(settings.idle_action)
        } else {
            None
        }
    }
}

/// 本平台能否"运行安装包、等它结束、再移到回收站"
pub const AUTO_INSTALL_SUPPORTED: bool = cfg!(any(windows, target_os = "macos"));

/// 按扩展名判断是不是本平台的安装包：Windows 的 exe/msi，macOS 的 dmg/pkg
pub fn is_installer(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if cfg!(windows) {
        matches!(ext.as_str(), "exe" | "msi")
    } else if cfg!(target_os = "macos") {
        matches!(ext.as_str(), "dmg" | "pkg")
    } else {
        false
    }
}

const INSTALL_TASKS_KEY: &str = "auto_install_tasks";

struct Work {
    path: PathBuf,
    settings: CompletionSettings,
    cancel: CancellationToken,
    install: bool,
}
pub struct Outcome {
    pub path: PathBuf,
    pub result: Result<(), String>,
    show_dialog: bool,
    /// 安装包已运行完并移到了回收站
    pub installed: bool,
}
pub struct CompletionActions {
    pub settings: CompletionSettings,
    pub countdown: Countdown,
    pub dialogs: VecDeque<PathBuf>,
    tx: Option<SyncSender<Work>>,
    rx: Receiver<Outcome>,
    pending: usize,
    cancel: CancellationToken,
    power: Option<Receiver<Result<(), String>>>,
    /// 扫描失败后必须由用户处理，不能继续自动关机。
    pub failed: bool,
    /// 勾选了"安装包运行后移到回收站"、尚未完成的任务；持久化，重启后续传完成仍然生效
    install_tasks: BTreeSet<u64>,
}
impl CompletionActions {
    pub fn new(store: &Store) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Work>(128);
        let (done, results) = mpsc::channel();
        let worker = std::thread::Builder::new().name("swoop-completion".into()).spawn(move || {
            while let Ok(work) = rx.recv() {
                if work.install {
                    // 安装程序可能要跑很久，单独起线程等它，不挡住其他文件的完成动作
                    let scanned = scan_file(&work.path, &work.settings, &work.cancel);
                    let installer_done = done.clone();
                    let path = work.path.clone();
                    let spawned = std::thread::Builder::new().name("swoop-installer".into()).spawn(move || {
                        let result = scanned.and_then(|path| install_and_trash(&path, &work.cancel));
                        let installed = result.is_ok();
                        let _ = installer_done.send(Outcome { path: work.path, result, show_dialog: false, installed });
                    });
                    if let Err(error) = spawned {
                        let result = Err(swoop_i18n::trf!("无法启动安装线程：{error}"));
                        let _ = done.send(Outcome { path, result, show_dialog: false, installed: false });
                    }
                    continue;
                }
                let result = process_file(&work.path, &work.settings, &work.cancel);
                let outcome = Outcome { path: work.path, result, show_dialog: work.settings.show_dialog, installed: false };
                if done.send(outcome).is_err() {
                    break;
                }
            }
        });
        Self {
            settings: CompletionSettings::load(store),
            countdown: Countdown::default(),
            dialogs: VecDeque::new(),
            tx: worker.ok().map(|_| tx),
            rx: results,
            pending: 0,
            cancel: CancellationToken::new(),
            power: None,
            failed: false,
            install_tasks: store
                .get_setting(INSTALL_TASKS_KEY)
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default(),
        }
    }
    /// 记下这个任务完成后要运行安装包并清理
    pub fn mark_install(&mut self, store: &Store, id: TaskId) {
        if self.install_tasks.insert(id.0) {
            self.save_install_tasks(store);
        }
    }
    /// 任务完成或删除时取出标记；返回它是否勾选过
    pub fn take_install(&mut self, store: &Store, id: TaskId) -> bool {
        let marked = self.install_tasks.remove(&id.0);
        if marked {
            self.save_install_tasks(store);
        }
        marked
    }
    fn save_install_tasks(&self, store: &Store) {
        let saved = serde_json::to_string(&self.install_tasks)
            .map_err(|e| e.to_string())
            .and_then(|v| store.set_setting(INSTALL_TASKS_KEY, &v).map_err(|e| e.to_string()));
        if let Err(error) = saved {
            tracing::warn!(%error, "保存安装包标记失败");
        }
    }
    pub fn busy(&self) -> bool {
        self.pending > 0 || self.power.is_some()
    }
    /// `dialog` 为 false 时不弹完成窗口（该任务的进度窗口会原地变成完成窗口）；
    /// `install` 为 true 时（扫描通过后）运行安装包，结束后移到回收站，不再按"每个文件完成"打开
    pub fn submit(&mut self, path: PathBuf, dialog: bool, install: bool) -> Result<(), String> {
        self.countdown.completed();
        if !install && self.settings.file_action == FileAction::None && self.settings.scanner.is_none() {
            if dialog {
                self.offer_dialog(path);
            }
            return Ok(());
        }
        if self.cancel.is_cancelled() {
            self.cancel = CancellationToken::new();
        }
        let mut settings = self.settings.clone();
        settings.show_dialog &= dialog;
        let work = Work { path, settings, cancel: self.cancel.clone(), install };
        let result = self
            .tx
            .as_ref()
            .ok_or_else(|| swoop_i18n::tr!("完成动作线程不可用").to_string())
            .and_then(|tx| tx.try_send(work).map_err(|_| swoop_i18n::tr!("完成动作队列已满或不可用").to_string()));
        if result.is_ok() {
            self.pending += 1;
        } else {
            self.failed = true;
        }
        result
    }
    fn offer_dialog(&mut self, path: PathBuf) {
        if self.settings.show_dialog {
            if self.dialogs.len() == 128 {
                self.dialogs.pop_front();
            }
            self.dialogs.push_back(path);
        }
    }
    pub fn poll(&mut self) -> Vec<Outcome> {
        let mut out = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            if result.result.is_err() {
                self.failed = true;
            } else if result.show_dialog {
                self.offer_dialog(result.path.clone());
            }
            out.push(result);
        }
        out
    }
    pub fn cancel_all(&mut self) {
        self.countdown.cancel();
        self.cancel.cancel();
    }
    pub fn open_path(&mut self, path: PathBuf, action: FileAction) -> Result<(), String> {
        if self.cancel.is_cancelled() {
            self.cancel = CancellationToken::new();
        }
        let work = Work {
            path,
            settings: CompletionSettings { file_action: action, ..Default::default() },
            cancel: self.cancel.clone(),
            install: false,
        };
        self.tx
            .as_ref()
            .ok_or(swoop_i18n::tr!("完成动作线程不可用"))?
            .try_send(work)
            .map_err(|_| swoop_i18n::tr!("完成动作队列已满或不可用"))?;
        self.pending += 1;
        Ok(())
    }
    pub fn request_shutdown(&mut self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("swoop-power".into())
            .spawn(move || {
                let _ = tx.send(shutdown());
            })
            .map_err(|e| e.to_string())?;
        self.power = Some(rx);
        Ok(())
    }
    pub fn poll_shutdown(&mut self) -> Option<Result<(), String>> {
        match self.power.as_ref()?.try_recv() {
            Ok(result) => {
                self.power = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.power = None;
                Some(Err(swoop_i18n::tr!("关机请求未完成").into()))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}
impl Drop for CompletionActions {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn regular_file(path: &Path) -> Result<PathBuf, String> {
    let metadata = path.symlink_metadata().map_err(|_| swoop_i18n::tr!("下载文件不存在或无法读取").to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(swoop_i18n::tr!("完成动作只接受普通文件").into());
    }
    path.canonicalize().map_err(|_| swoop_i18n::tr!("无法解析下载文件路径").into())
}
fn scanner_command(scanner: &Scanner, path: &Path) -> Command {
    let mut command = Command::new(&scanner.program);
    // OsString 保留非 UTF-8 文件名；每一行始终是一个参数，空格/引号不再拆分。
    for template in &scanner.arguments {
        let mut argument = OsString::new();
        for (index, part) in template.split("{file}").enumerate() {
            if index > 0 {
                argument.push(path);
            }
            argument.push(part);
        }
        command.arg(argument);
    }
    command
}
fn wait_command(mut command: Command, timeout: Duration, cancel: &CancellationToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        return Err(swoop_i18n::tr!("动作已取消").into());
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|e| swoop_i18n::trf!("无法启动程序：{e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(swoop_i18n::trf!("程序未成功结束（退出码 {:?}）", status.code()))
                };
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(swoop_i18n::trf!("无法读取程序状态：{error}"));
            }
        }
        if cancel.is_cancelled() || started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancel.is_cancelled() {
                swoop_i18n::tr!("动作已取消")
            } else {
                swoop_i18n::tr!("程序运行超时")
            }
            .into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
/// 配置了扫描程序就先扫描；扫描可能隔离/删除文件，返回重新检查过的路径。
fn scan_file(path: &Path, settings: &CompletionSettings, cancel: &CancellationToken) -> Result<PathBuf, String> {
    let path = regular_file(path)?;
    if let Some(scanner) = &settings.scanner {
        wait_command(scanner_command(scanner, &path), Duration::from_secs(scanner.timeout_secs), cancel)?;
    }
    if cancel.is_cancelled() {
        return Err(swoop_i18n::tr!("动作已取消").into());
    }
    regular_file(&path)
}
fn process_file(path: &Path, settings: &CompletionSettings, cancel: &CancellationToken) -> Result<(), String> {
    let path = scan_file(path, settings, cancel)?;
    if settings.file_action != FileAction::None {
        open_file(&regular_file(&path)?, settings.file_action, cancel)?;
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn open_file(path: &Path, action: FileAction, cancel: &CancellationToken) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("/usr/bin/open");
        if action == FileAction::Reveal {
            command.arg("-R");
        }
        command.arg("--").arg(path);
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(if action == FileAction::Reveal { path.parent().unwrap_or(path) } else { path });
        command
    };
    command.env_remove("DYLD_INSERT_LIBRARIES");
    wait_command(command, Duration::from_secs(15), cancel)
}
#[cfg(target_os = "windows")]
fn open_file(path: &Path, action: FileAction, _cancel: &CancellationToken) -> Result<(), String> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut c_void;
    }
    let path = if action == FileAction::Reveal { path.parent().unwrap_or(path) } else { path };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    // 参数为空；文件路径不会交给 cmd.exe / PowerShell 解释。
    let result = unsafe {
        ShellExecuteW(std::ptr::null_mut(), operation.as_ptr(), wide.as_ptr(), std::ptr::null(), std::ptr::null(), 1)
    } as isize;
    if result > 32 { Ok(()) } else { Err(swoop_i18n::trf!("系统无法打开文件（{result}）")) }
}

/// 运行安装包并等它结束，再把安装包移到回收站。取消（如退出 Swoop）只停止等待，不删除。
fn install_and_trash(path: &Path, cancel: &CancellationToken) -> Result<(), String> {
    if !is_installer(path) {
        return Err(swoop_i18n::tr!("不是本系统的安装包，未运行").into());
    }
    run_installer(path, cancel)?;
    if cancel.is_cancelled() {
        return Err(swoop_i18n::tr!("动作已取消").into());
    }
    move_to_trash(&regular_file(path)?)
}

/// 安装程序最长等一天；超过就放弃等待，安装包保留
const INSTALLER_WAIT: Duration = Duration::from_secs(24 * 3600);

fn wait_until(cancel: &CancellationToken, timeout: Duration, mut done: impl FnMut() -> Result<bool, String>) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if done()? {
            return Ok(());
        }
        if cancel.is_cancelled() {
            return Err(swoop_i18n::tr!("动作已取消").into());
        }
        if started.elapsed() >= timeout {
            return Err(swoop_i18n::tr!("等待安装程序超时，安装包未删除").into());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(target_os = "macos")]
fn run_installer(path: &Path, cancel: &CancellationToken) -> Result<(), String> {
    let open = |wait: bool| {
        let mut command = Command::new("/usr/bin/open");
        if wait {
            command.arg("-W");
        }
        command.arg("--").arg(path).env_remove("DYLD_INSERT_LIBRARIES");
        command
    };
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pkg")) {
        // open -W 等"安装器"退出
        return wait_command(open(true), INSTALLER_WAIT, cancel);
    }
    // dmg：挂载器挂好就退出了，要等用户装完、推出磁盘映像后再删
    wait_command(open(false), Duration::from_secs(15), cancel)?;
    wait_until(cancel, Duration::from_secs(120), || dmg_attached(path)).map_err(|error| {
        if cancel.is_cancelled() { error } else { swoop_i18n::tr!("磁盘映像未挂载，安装包未删除").into() }
    })?;
    wait_until(cancel, INSTALLER_WAIT, || dmg_attached(path).map(|attached| !attached))
}

#[cfg(target_os = "macos")]
fn dmg_attached(path: &Path) -> Result<bool, String> {
    let output = Command::new("/usr/bin/hdiutil")
        .arg("info")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| swoop_i18n::trf!("无法查询磁盘映像：{e}"))?;
    let target = path.to_string_lossy();
    Ok(String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.trim_start().starts_with("image-path") && line.split_once(':').is_some_and(|(_, v)| v.trim() == target)
    }))
}

#[cfg(target_os = "macos")]
fn move_to_trash(path: &Path) -> Result<(), String> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|error| swoop_i18n::trf!("无法移到废纸篓：{}", error.localizedDescription()))
}

/// Shell API 不认 canonicalize 产生的 `\\?\` 前缀
#[cfg(windows)]
fn shell_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let text = path.as_os_str().to_string_lossy();
    let plain = text.strip_prefix(r"\\?\").filter(|rest| !rest.starts_with("UNC\\")).map(std::ffi::OsStr::new);
    plain.unwrap_or(path.as_os_str()).encode_wide().chain(Some(0)).collect()
}

#[cfg(windows)]
fn run_installer(path: &Path, cancel: &CancellationToken) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    let file = shell_wide(path);
    let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        nShow: 1,
        ..Default::default()
    };
    // 不经过 cmd.exe / PowerShell；需要管理员权限的安装包由系统弹 UAC。
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(swoop_i18n::trf!("系统无法运行安装包（{}）", std::io::Error::last_os_error()));
    }
    if info.hProcess.is_null() {
        return Err(swoop_i18n::tr!("无法跟踪安装程序进程，安装包未删除").into());
    }
    let started = Instant::now();
    let result = loop {
        match unsafe { WaitForSingleObject(info.hProcess, 1000) } {
            WAIT_OBJECT_0 => break Ok(()),
            WAIT_TIMEOUT if cancel.is_cancelled() => break Err(swoop_i18n::tr!("动作已取消").to_string()),
            WAIT_TIMEOUT if started.elapsed() >= INSTALLER_WAIT => {
                break Err(swoop_i18n::tr!("等待安装程序超时，安装包未删除").to_string());
            }
            WAIT_TIMEOUT => {}
            other => break Err(swoop_i18n::trf!("无法等待安装程序结束（{other}）")),
        }
    };
    unsafe {
        CloseHandle(info.hProcess);
    }
    result
}

#[cfg(windows)]
fn move_to_trash(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW,
    };
    // pFrom 是以两个 0 结尾的列表
    let mut from = shell_wide(path);
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16,
        ..Default::default()
    };
    let result = unsafe { SHFileOperationW(&mut op) };
    if result == 0 && op.fAnyOperationsAborted == 0 {
        Ok(())
    } else {
        Err(swoop_i18n::trf!("无法移到回收站（{result}）"))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run_installer(_: &Path, _: &CancellationToken) -> Result<(), String> {
    Err(swoop_i18n::tr!("当前系统不支持自动运行安装包").into())
}
#[cfg(not(any(windows, target_os = "macos")))]
fn move_to_trash(_: &Path) -> Result<(), String> {
    Err(swoop_i18n::tr!("当前系统不支持自动运行安装包").into())
}

/// 仅倒计时到期后调用。测试不执行此函数，避免改变开发机器状态。
pub fn shutdown() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let command = {
        let mut c = Command::new("/usr/bin/osascript");
        c.args(["-e", "tell application \"System Events\" to shut down"]);
        c
    };
    #[cfg(target_os = "windows")]
    let command = {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let mut c = Command::new(root.join("System32").join("shutdown.exe"));
        // 倒计时在应用内；Windows /t > 0 会隐含 /f，因此这里只用 /t 0，不强制关闭其他程序。
        c.args(["/s", "/t", "0"]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let command = {
        let mut c = Command::new("systemctl");
        c.arg("poweroff");
        c
    };
    wait_command(command, Duration::from_secs(30), &CancellationToken::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_install_choice_and_marked_tasks_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.db");
        let store = Store::open(&path).unwrap();
        // 旧版本保存的设置没有 auto_install 字段，读出来默认不勾选
        store.set_setting("completion_actions", r#"{"file_action":"Reveal"}"#).unwrap();
        let mut settings = CompletionSettings::load(&store);
        assert_eq!(settings.file_action, FileAction::Reveal);
        assert!(!settings.auto_install);
        settings.set_auto_install(&store, true).unwrap();

        let mut actions = CompletionActions::new(&store);
        actions.mark_install(&store, TaskId(7));
        drop(actions);
        drop(store);

        let store = Store::open(&path).unwrap();
        assert!(CompletionSettings::load(&store).auto_install);
        let mut actions = CompletionActions::new(&store);
        assert!(actions.take_install(&store, TaskId(7)));
        assert!(!actions.take_install(&store, TaskId(7)), "取出后不再生效");
        assert!(!CompletionActions::new(&store).take_install(&store, TaskId(7)));
    }
    #[test]
    fn installer_extensions_follow_platform() {
        let check = |name: &str| is_installer(Path::new(name));
        assert!(!check("photo.jpg"));
        assert_eq!(check("setup.EXE"), cfg!(windows));
        assert_eq!(check("tool.msi"), cfg!(windows));
        assert_eq!(check("App.dmg"), cfg!(target_os = "macos"));
        assert_eq!(check("App.pkg"), cfg!(target_os = "macos"));
    }
    /// 会弹出 Finder 窗口并往废纸篓里放一个小 dmg，默认不跑：
    /// cargo test -p swoop-app --bin swoop real_dmg -- --ignored
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn real_dmg_is_trashed_after_it_is_ejected() {
        let dir = tempfile::tempdir().unwrap();
        let dmg = dir.path().join("SwoopAutoInstallTest.dmg");
        let created = Command::new("/usr/bin/hdiutil")
            .args(["create", "-size", "1m", "-fs", "HFS+", "-volname", "SwoopAutoInstallTest"])
            .arg(&dmg)
            .status()
            .unwrap();
        assert!(created.success());
        let dmg = dmg.canonicalize().unwrap();
        let worker = {
            let dmg = dmg.clone();
            std::thread::spawn(move || install_and_trash(&dmg, &CancellationToken::new()))
        };
        wait_until(&CancellationToken::new(), Duration::from_secs(30), || dmg_attached(&dmg)).unwrap();
        assert!(dmg.exists(), "挂载期间不能删除");
        let ejected = Command::new("/usr/bin/hdiutil").args(["detach", "/Volumes/SwoopAutoInstallTest"]).status();
        assert!(ejected.unwrap().success());
        worker.join().unwrap().unwrap();
        assert!(!dmg.exists());
    }
    #[test]
    fn non_installers_are_never_run_or_trashed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(install_and_trash(&file, &CancellationToken::new()).is_err());
        assert!(file.exists());
    }
    #[test]
    fn settings_default_off_and_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.db");
        let store = Store::open(&path).unwrap();
        let mut s = CompletionSettings::load(&store);
        assert_eq!(s, CompletionSettings::default());
        s.file_action = FileAction::Reveal;
        s.show_dialog = true;
        s.idle_action = IdleAction::Quit;
        s.save(&store).unwrap();
        drop(store);
        assert_eq!(CompletionSettings::load(&Store::open(&path).unwrap()), s);
    }
    #[test]
    fn scanner_requires_file_placeholder_and_bounded_timeout() {
        let mut s = CompletionSettings {
            scanner: Some(Scanner {
                program: std::env::current_exe().unwrap(),
                arguments: vec!["--scan".into()],
                timeout_secs: 300,
            }),
            ..Default::default()
        };
        assert!(s.validate(true).is_err());
        s.scanner.as_mut().unwrap().arguments.push("{file}".into());
        assert!(s.validate(true).is_ok());
        s.scanner.as_mut().unwrap().timeout_secs = 0;
        assert!(s.validate(true).is_err());
    }
    #[test]
    fn parameters_preserve_spaces_quotes_and_shell_literals() {
        let scanner = Scanner {
            arguments: vec!["--input={file}".into(), "literal $(touch x); \"quoted\"".into(), "{file}{file}".into()],
            ..Default::default()
        };
        let cmd = scanner_command(&scanner, Path::new("/a b/文件;$(x).bin"));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            ["--input=/a b/文件;$(x).bin", "literal $(touch x); \"quoted\"", "/a b/文件;$(x).bin/a b/文件;$(x).bin"]
        );
    }
    #[test]
    fn countdown_needs_new_success_resets_for_work_and_is_cancellable() {
        let mut state = Countdown::default();
        let now = Instant::now();
        let settings = CompletionSettings { idle_action: IdleAction::Quit, countdown_secs: 10, ..Default::default() };
        assert_eq!(state.tick(now, true, &settings), None);
        assert_eq!(state.remaining(now), None);
        state.completed();
        state.tick(now, true, &settings);
        assert!(state.remaining(now).is_some());
        state.tick(now + Duration::from_secs(9), false, &settings);
        assert_eq!(state.remaining(now), None);
        state.tick(now + Duration::from_secs(20), true, &settings);
        assert_eq!(state.tick(now + Duration::from_secs(29), true, &settings), None);
        assert_eq!(state.tick(now + Duration::from_secs(30), true, &settings), Some(IdleAction::Quit));
        assert_eq!(state.tick(now + Duration::from_secs(40), true, &settings), None);
        state.completed();
        state.tick(now, true, &settings);
        state.cancel();
        assert_eq!(state.tick(now + Duration::from_secs(60), true, &settings), None);
    }
    #[test]
    fn deleted_and_symbolic_files_do_not_trigger_actions() {
        let dir = tempfile::tempdir().unwrap();
        assert!(regular_file(&dir.path().join("missing")).is_err());
        assert!(regular_file(dir.path()).is_err());
        #[cfg(unix)]
        {
            let target = dir.path().join("target");
            std::fs::write(&target, "data").unwrap();
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(target, &link).unwrap();
            assert!(regular_file(&link).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn scanner_exit_failure_blocks_open_and_timeout_reaps_child() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.bin");
        std::fs::write(&file, "x").unwrap();
        let settings = CompletionSettings {
            scanner: Some(Scanner { program: "/usr/bin/false".into(), ..Default::default() }),
            file_action: FileAction::Open,
            ..Default::default()
        };
        assert!(process_file(&file, &settings, &CancellationToken::new()).unwrap_err().contains("退出码"));
        let mut command = Command::new("/bin/sleep");
        command.arg("10");
        let started = Instant::now();
        assert_eq!(
            wait_command(command, Duration::from_millis(50), &CancellationToken::new()).unwrap_err(),
            "程序运行超时"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            wait_command(Command::new("/usr/bin/true"), Duration::from_secs(1), &cancel).unwrap_err(),
            "动作已取消"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_a_running_scanner_does_not_wait_for_its_timeout() {
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let started = Instant::now();
        let worker = std::thread::spawn(move || {
            let mut command = Command::new("/bin/sleep");
            command.arg("10");
            wait_command(command, Duration::from_secs(20), &token)
        });
        std::thread::sleep(Duration::from_millis(100));
        cancel.cancel();
        assert_eq!(worker.join().unwrap().unwrap_err(), "动作已取消");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
