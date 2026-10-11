//! Command-line parsing is shared by the GUI binary and the Windows console binary.
use std::ffi::OsString;
use std::path::PathBuf;

use crate::instance::{self, Command, ConflictAction};

const HELP: &str = "Swoop 命令行\n\n用法：swoop-cli [--data-dir PATH] [--json] COMMAND\n  --help, -h             显示帮助\n  --version, -V          显示版本\n  status                 输出任务与队列 JSON（不含 URL 和请求头）\n  show                   显示已有桌面窗口\n  resolve ID --action overwrite|rename|skip\n                         处理单个同名冲突\n  conflict-policy [ask|rename|overwrite|skip]\n                         查询或保存同名文件默认策略\n  add URL [--dir PATH] [--name NAME] [--queue ID]\n                         将 HTTP(S)/FTP(S) 链接加入已有实例\n  pause ID               暂停任务\n  resume ID              继续任务\n  queue-start ID         启动队列\n  queue-stop ID          停止队列及其在途任务\n  media-add URL --output FILE [--duration SEC] [--max-height N] [--audio-language TAG] [--speed-limit BPS]\n  media-status [ID]       查询媒体任务（不含源链接/请求头）\n  media-stop ID          停止媒体任务\n  media-limit ID BPS     设置媒体任务限速，0 不限速\n  media-retry ID         继续已停止/失败媒体任务\n  media-resolve ID --action overwrite|rename|skip\n                         处理媒体同名冲突\n  grabber-create URL --dir PATH [--depth N] [--pages N] [--bytes N] [--concurrency N]\n                         创建项目（不开始抓取）\n  grabber-open PATH      打开已有项目\n  grabber-run            运行当前项目\n  grabber-stop           停止当前抓取\n  grabber-status         查询当前项目及最近异步操作\n\n所有控制命令均输出 JSON。没有实例时返回非零，不启动 GUI、不写数据库。\nWindows 脚本请使用 swoop-cli.exe；swoop.exe 保留无控制台桌面启动方式。\n";

#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Help(bool),
    Version(bool),
    Request { data: PathBuf, command: Command },
}

fn text(value: OsString) -> Result<String, String> {
    value.into_string().map_err(|_| swoop_i18n::tr!("命令与选项值必须是有效 Unicode").into())
}
fn id(value: OsString) -> Result<u64, String> {
    text(value)?.parse::<u64>().ok().filter(|id| *id > 0).ok_or_else(|| swoop_i18n::tr!("ID 必须是正整数").into())
}
fn absolute(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir().map(|cwd| cwd.join(path)).map_err(|_| swoop_i18n::tr!("无法读取当前目录").into())
    }
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, String> {
    let mut args = args.into_iter();
    let mut data = instance::data_dir();
    let mut json = false;
    let mut rest = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            data = args
                .next()
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .ok_or(swoop_i18n::tr!("--data-dir 需要路径"))?;
        } else if arg == "--json" {
            json = true;
        } else {
            rest.push(arg);
        }
    }
    let mut rest = rest.into_iter();
    let Some(command) = rest.next() else {
        return Ok(Invocation::Help(json));
    };
    let command = text(command)?;
    let command = match command.as_str() {
        "--help" | "-h" | "help" => {
            if rest.next().is_some() {
                return Err(swoop_i18n::tr!("帮助命令不接受额外参数").into());
            }
            return Ok(Invocation::Help(json));
        }
        "--version" | "-V" => {
            if rest.next().is_some() {
                return Err(swoop_i18n::tr!("版本命令不接受额外参数").into());
            }
            return Ok(Invocation::Version(json));
        }
        "status" => Command::Status,
        "show" => Command::Show,
        "resolve" | "media-resolve" => {
            let id = id(rest.next().ok_or(swoop_i18n::tr!("需要任务 ID"))?)?;
            if rest.next().as_deref() != Some(std::ffi::OsStr::new("--action")) {
                return Err(swoop_i18n::tr!("resolve 需要 --action").into());
            }
            let action = ConflictAction::parse(&text(rest.next().ok_or(swoop_i18n::tr!("--action 需要策略"))?)?)?;
            let command = if command == "media-resolve" {
                Command::MediaResolve { id, action }
            } else {
                Command::Resolve { id, action }
            };
            command.validate()?;
            command
        }
        "conflict-policy" => Command::ConflictPolicy {
            policy: rest.next().map(text).transpose()?.map(|v| ConflictAction::parse(&v)).transpose()?,
        },
        "pause" | "resume" | "queue-start" | "queue-stop" => {
            let id = id(rest.next().ok_or(swoop_i18n::tr!("命令需要 ID"))?)?;
            match command.as_str() {
                "pause" => Command::Pause { id },
                "resume" => Command::Resume { id },
                "queue-start" => Command::QueueStart { id },
                _ => Command::QueueStop { id },
            }
        }
        "media-add" | "media-status" | "media-stop" | "media-retry" | "media-limit" | "grabber-create"
        | "grabber-open" | "grabber-run" | "grabber-stop" | "grabber-status" => {
            let command = parse_desktop(&command, rest.collect())?;
            command.validate()?;
            return Ok(Invocation::Request { data: absolute(data)?, command });
        }
        "add" => {
            let url = text(rest.next().ok_or(swoop_i18n::tr!("add 需要 HTTP(S)/FTP(S) 链接"))?)?;
            instance::download_url(&url)?;
            let (mut directory, mut name, mut queue) = (None, None, None);
            while let Some(option) = rest.next() {
                let option = text(option)?;
                let value = rest.next().ok_or_else(|| swoop_i18n::trf!("{option} 需要值"))?;
                match option.as_str() {
                    "--dir" if directory.is_none() => directory = Some(absolute(PathBuf::from(value))?),
                    "--name" if name.is_none() => name = Some(text(value)?),
                    "--queue" if queue.is_none() => queue = Some(id(value)?),
                    _ => return Err(swoop_i18n::trf!("未知或重复选项：{option}")),
                }
            }
            return Ok(Invocation::Request {
                data: absolute(data)?,
                command: Command::Add { url, directory, name, queue },
            });
        }
        _ => return Err(swoop_i18n::trf!("未知命令：{command}；使用 --help 查看帮助")),
    };
    if rest.next().is_some() {
        return Err(swoop_i18n::tr!("命令包含多余参数").into());
    }
    Ok(Invocation::Request { data: absolute(data)?, command })
}

fn required_path(value: OsString) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Err(swoop_i18n::tr!("路径不能为空").into());
    }
    absolute(PathBuf::from(value))
}

fn parse_desktop(command: &str, args: Vec<OsString>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let result = match command {
        "media-status" => Command::MediaStatus { id: args.next().map(id).transpose()? },
        "media-limit" => Command::MediaLimit {
            id: id(args.next().ok_or(swoop_i18n::tr!("需要媒体任务 ID"))?)?,
            speed_limit_bps: number(args.next())?.ok_or(swoop_i18n::tr!("需要限速值，0 表示不限速"))?,
        },
        "media-stop" => Command::MediaStop { id: id(args.next().ok_or(swoop_i18n::tr!("需要媒体任务 ID"))?)? },
        "media-retry" => Command::MediaRetry { id: id(args.next().ok_or(swoop_i18n::tr!("需要媒体任务 ID"))?)? },
        "grabber-open" => Command::GrabberOpen {
            directory: required_path(args.next().ok_or(swoop_i18n::tr!("需要项目目录"))?)?,
        },
        "grabber-run" => Command::GrabberRun,
        "grabber-stop" => Command::GrabberStop,
        "grabber-status" => Command::GrabberStatus,
        "media-add" | "grabber-create" => {
            let url = text(args.next().ok_or(swoop_i18n::tr!("需要下载链接"))?)?;
            if command == "media-add" {
                instance::media_url(&url)?;
            } else {
                instance::network_url(&url)?;
            }
            let allowed: &[&str] = if command == "media-add" {
                &["--output", "--duration", "--max-height", "--audio-language", "--speed-limit"]
            } else {
                &["--dir", "--depth", "--pages", "--bytes", "--concurrency"]
            };
            let mut options = std::collections::BTreeMap::new();
            while let Some(option) = args.next() {
                let option = text(option)?;
                if !allowed.contains(&option.as_str()) || options.contains_key(&option) {
                    return Err(swoop_i18n::trf!("未知或重复选项：{option}"));
                }
                let value =
                    args.next().filter(|v| !v.is_empty()).ok_or_else(|| swoop_i18n::trf!("{option} 需要非空值"))?;
                options.insert(option, value);
            }
            if command == "media-add" {
                Command::MediaAdd {
                    url,
                    output: required_path(
                        options.remove("--output").ok_or(swoop_i18n::tr!("media-add 需要 --output"))?,
                    )?,
                    duration: number(options.remove("--duration"))?,
                    max_height: number(options.remove("--max-height"))?,
                    audio_language: options.remove("--audio-language").map(text).transpose()?,
                    speed_limit_bps: number(options.remove("--speed-limit"))?,
                }
            } else {
                Command::GrabberCreate {
                    url,
                    directory: required_path(
                        options.remove("--dir").ok_or(swoop_i18n::tr!("grabber-create 需要 --dir"))?,
                    )?,
                    max_depth: number(options.remove("--depth"))?,
                    max_pages: number(options.remove("--pages"))?,
                    max_bytes: number(options.remove("--bytes"))?,
                    concurrency: number(options.remove("--concurrency"))?,
                }
            }
        }
        _ => return Err(swoop_i18n::tr!("未知命令").into()),
    };
    if args.next().is_some() {
        return Err(swoop_i18n::tr!("命令包含多余参数").into());
    }
    Ok(result)
}

fn number<T: std::str::FromStr>(value: Option<OsString>) -> Result<Option<T>, String> {
    value.map(|v| text(v)?.parse().map_err(|_| swoop_i18n::tr!("选项需要范围内的整数").into())).transpose()
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> u8 {
    let invocation = match parse(args) {
        Ok(invocation) => invocation,
        Err(error) => {
            println!("{}", instance::failure("usage", &error));
            return 2;
        }
    };
    match invocation {
        Invocation::Help(json) => {
            if json {
                println!("{}", serde_json::json!({"ok":true,"data":{"help":HELP}}));
            } else {
                print!("{HELP}");
            }
            0
        }
        Invocation::Version(json) => {
            if json {
                println!("{}", serde_json::json!({"ok":true,"data":{"version":env!("CARGO_PKG_VERSION")}}));
            } else {
                println!("swoop {}", env!("CARGO_PKG_VERSION"));
            }
            0
        }
        Invocation::Request { data, command } => {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(_) => {
                    println!("{}", instance::failure("unavailable", "无法初始化控制客户端"));
                    return 3;
                }
            };
            match runtime.block_on(instance::send(&data, &command, false)) {
                Ok(response) => {
                    let code = if response.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                        0
                    } else if response["error"]["code"] == "unconfirmed" {
                        3
                    } else {
                        4
                    };
                    println!("{response}");
                    code
                }
                Err(error) => {
                    println!("{}", instance::failure("unavailable", &error));
                    3
                }
            }
        }
    }
}

/// Attach only for arguments passed to the GUI executable. The separate console
/// binary remains the supported Windows scripting entry, including exit codes.
pub fn attach_parent_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{
            ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
        };
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
        if stdout.is_null() || stdout == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mms_requires_explicit_duration_and_mkv_without_accepting_credentials() {
        let mkv = std::env::temp_dir().join("media.mkv").to_string_lossy().into_owned();
        let mp4 = std::env::temp_dir().join("media.mp4").to_string_lossy().into_owned();
        for scheme in ["mms", "mmsh", "mmst"] {
            let command = vec![
                format!("{scheme}://media.test/live"),
                "--output".into(),
                mkv.clone(),
                "--duration".into(),
                "10".into(),
            ];
            assert!(
                parse_desktop("media-add", command.into_iter().map(OsString::from).collect())
                    .unwrap()
                    .validate()
                    .is_ok()
            );
        }
        for args in [
            vec!["mms://media.test/live", "--output", &mp4, "--duration", "10"],
            vec!["mms://media.test/live", "--output", &mkv],
            vec!["mms://user:password@media.test/live", "--output", &mkv, "--duration", "10"],
        ] {
            let result = parse_desktop("media-add", args.into_iter().map(OsString::from).collect())
                .and_then(|command| command.validate());
            assert!(result.is_err());
        }
    }
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn strict_parser_and_absolute_paths() {
        assert_eq!(parse(args(&["--json", "--help"])).unwrap(), Invocation::Help(true));
        assert!(parse(args(&["pause", "0"])).is_err());
        assert!(parse(args(&["status", "ignored"])).is_err());
        assert!(parse(args(&["add", "file:///tmp/a"])).is_err());
        assert!(parse(args(&["add", "https://user:password@example.com/a"])).is_err());
        assert!(parse(args(&["add", "https://example.com/a", "--dir", "a", "--dir", "b"])).is_err());
        let Invocation::Request { data, command: Command::Add { directory: Some(directory), queue, .. } } =
            parse(args(&[
                "--data-dir",
                "状态 空间",
                "add",
                "https://example.com/a",
                "--dir",
                "下载 空间",
                "--queue",
                "3",
            ]))
            .unwrap()
        else {
            panic!("add request");
        };
        assert!(data.is_absolute() && directory.is_absolute());
        assert_eq!(queue, Some(3));
    }
    #[test]
    fn ftp_add_is_supported_without_url_credentials_or_media_protocol_expansion() {
        for url in ["ftp://example.test/file.zip", "ftps://example.test/file.zip"] {
            assert!(matches!(
                parse(args(&["add", url])).unwrap(),
                Invocation::Request { command: Command::Add { .. }, .. }
            ));
            assert!(parse(args(&["media-add", url, "--output", "a.mp4"])).is_err());
        }
        for url in [
            "ftp://user:secret@example.test/file.zip",
            "ftp://@example.test/file.zip",
            "ftp://example.test/dir/",
            "ftp://example.test/file;type=a",
        ] {
            assert!(parse(args(&["add", url])).is_err());
        }
    }
    #[test]
    fn media_and_grabber_arguments_are_strict_and_paths_are_absolute() {
        for values in [
            vec!["media-add", "https://example.test/a.m3u8"],
            vec!["media-add", "https://example.test/a.m3u8", "--output", "a.mp4", "--duration", "0"],
            vec!["media-add", "https://example.test/a.m3u8", "--output", "a.mp4", "--max-height", "9000"],
            vec!["media-add", "https://example.test/a.m3u8", "--output", "a.mp4", "--audio-language", "en\r\n"],
            vec!["media-add", "https://user:pass@example.test/a.m3u8", "--output", "a.mp4"],
            vec!["media-status", "0"],
            vec!["media-stop", "1", "extra"],
            vec!["media-retry", "-2"],
            vec!["grabber-create", "https://example.test/", "--dir", "a", "--pages", "0"],
            vec!["grabber-create", "https://example.test/", "--dir", "a", "--concurrency", "17"],
            vec!["grabber-create", "https://example.test/", "--dir", "a", "--depth", "101"],
            vec!["grabber-create", "https://example.test/", "--dir", "a", "--bytes", "0"],
            vec!["grabber-open", ""],
            vec!["grabber-run", "unexpected"],
        ] {
            assert!(parse(args(&values)).is_err(), "{values:?}");
        }
        let Invocation::Request { command: Command::MediaAdd { output, duration, max_height, .. }, .. } =
            parse(args(&[
                "media-add",
                "https://example.test/a.m3u8",
                "--output",
                "中文 输出/a.mkv",
                "--duration",
                "30",
                "--max-height",
                "720",
            ]))
            .unwrap()
        else {
            panic!("media command");
        };
        assert!(output.is_absolute());
        assert_eq!(duration, Some(30));
        assert_eq!(max_height, Some(720));
        assert!(matches!(
            parse(args(&["media-status"])).unwrap(),
            Invocation::Request { command: Command::MediaStatus { id: None }, .. }
        ));
        assert!(matches!(
            parse(args(&["grabber-create", "https://example.test/", "--dir", "项目", "--depth", "0"])).unwrap(),
            Invocation::Request { command: Command::GrabberCreate { max_depth: Some(0), .. }, .. }
        ));
    }
    #[test]
    fn conflict_actions_require_an_explicit_non_ask_resolution() {
        for values in [
            vec!["resolve", "0", "--action", "rename"],
            vec!["resolve", "1", "--action", "ask"],
            vec!["media-resolve", "1", "--action", "ask"],
            vec!["resolve", "1", "overwrite"],
            vec!["conflict-policy", "unknown"],
            vec!["conflict-policy", "ask", "rename"],
        ] {
            assert!(parse(args(&values)).is_err(), "{values:?}");
        }
        assert!(matches!(
            parse(args(&["resolve", "7", "--action", "overwrite"])).unwrap(),
            Invocation::Request { command: Command::Resolve { id: 7, action: ConflictAction::Overwrite }, .. }
        ));
        assert!(matches!(
            parse(args(&["conflict-policy"])).unwrap(),
            Invocation::Request { command: Command::ConflictPolicy { policy: None }, .. }
        ));
        assert!(matches!(
            parse(args(&["conflict-policy", "ask"])).unwrap(),
            Invocation::Request { command: Command::ConflictPolicy { policy: Some(ConflictAction::Ask) }, .. }
        ));
        assert!(matches!(
            parse(args(&["media-resolve", "7", "--action", "skip"])).unwrap(),
            Invocation::Request { command: Command::MediaResolve { id: 7, action: ConflictAction::Skip }, .. }
        ));
    }
    #[test]
    fn media_rate_cli_accepts_zero_and_rejects_invalid_limits() {

        let output = std::env::temp_dir().join("media-rate-fixture.mp4").to_string_lossy().into_owned();
        let Invocation::Request { command: Command::MediaAdd { speed_limit_bps, .. }, .. } =
            parse(args(&["media-add", "https://media.test/playlist.m3u8", "--output", &output, "--speed-limit", "65536"]))
                .unwrap()
        else {
            panic!("expected media-add");
        };
        assert_eq!(speed_limit_bps, Some(65536));
        for value in ["0", "1", "18446744073709551615"] {
            assert!(matches!(
                parse(args(&["media-limit", "1", value])).unwrap(),
                Invocation::Request { command: Command::MediaLimit { .. }, .. }
            ));
        }
        for values in [
            vec!["media-limit", "0", "1"],
            vec!["media-limit", "1", "-1"],
            vec!["media-limit", "1", "NaN"],
            vec!["media-limit", "1"],
            vec!["media-limit", "1", "0", "extra"],
        ] {
            assert!(parse(args(&values)).is_err(), "{values:?}");
        }
    }
}
