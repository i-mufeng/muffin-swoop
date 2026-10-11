//! 下载后动作。配置默认关闭；外部程序在有界工作线程执行，不经过 shell。
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};
use swoop_core::Store;
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
}
impl Default for CompletionSettings {
    fn default() -> Self {
        Self {
            file_action: FileAction::None,
            scanner: None,
            show_dialog: false,
            idle_action: IdleAction::None,
            countdown_secs: 30,
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

struct Work {
    path: PathBuf,
    settings: CompletionSettings,
    cancel: CancellationToken,
}
pub struct Outcome {
    pub path: PathBuf,
    pub result: Result<(), String>,
    show_dialog: bool,
}
pub struct CompletionActions {
    pub settings: CompletionSettings,
    pub countdown: Countdown,
    pub dialogs: VecDeque<PathBuf>,
    pub needs_attention: bool,
    tx: Option<SyncSender<Work>>,
    rx: Receiver<Outcome>,
    pending: usize,
    cancel: CancellationToken,
    power: Option<Receiver<Result<(), String>>>,
    /// 扫描失败后必须由用户处理，不能继续自动关机。
    pub failed: bool,
}
impl CompletionActions {
    pub fn new(store: &Store) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Work>(128);
        let (done, results) = mpsc::channel();
        let worker = std::thread::Builder::new().name("swoop-completion".into()).spawn(move || {
            while let Ok(work) = rx.recv() {
                let result = process_file(&work.path, &work.settings, &work.cancel);
                if done.send(Outcome { path: work.path, result, show_dialog: work.settings.show_dialog }).is_err() {
                    break;
                }
            }
        });
        Self {
            settings: CompletionSettings::load(store),
            countdown: Countdown::default(),
            dialogs: VecDeque::new(),
            needs_attention: false,
            tx: worker.ok().map(|_| tx),
            rx: results,
            pending: 0,
            cancel: CancellationToken::new(),
            power: None,
            failed: false,
        }
    }
    pub fn busy(&self) -> bool {
        self.pending > 0 || self.power.is_some()
    }
    pub fn submit(&mut self, path: PathBuf) -> Result<(), String> {
        self.countdown.completed();
        if self.settings.file_action == FileAction::None && self.settings.scanner.is_none() {
            self.offer_dialog(path);
            return Ok(());
        }
        if self.cancel.is_cancelled() {
            self.cancel = CancellationToken::new();
        }
        let work = Work { path, settings: self.settings.clone(), cancel: self.cancel.clone() };
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
            self.needs_attention = true;
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
fn process_file(path: &Path, settings: &CompletionSettings, cancel: &CancellationToken) -> Result<(), String> {
    let path = regular_file(path)?;
    if let Some(scanner) = &settings.scanner {
        wait_command(scanner_command(scanner, &path), Duration::from_secs(scanner.timeout_secs), cancel)?;
    }
    if cancel.is_cancelled() {
        return Err(swoop_i18n::tr!("动作已取消").into());
    }
    // 扫描可能隔离/删除文件；重新检查后再交给系统打开。
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
