use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// 定位写入：各 worker 共享同一个 File，无需 seek，也无需加锁。
pub fn write_at(file: &File, buf: &[u8], pos: u64) -> (usize, io::Result<()>) {
    let mut written = 0usize;
    while written < buf.len() {
        #[cfg(unix)]
        let result = {
            use std::os::unix::fs::FileExt;
            file.write_at(&buf[written..], pos + written as u64)
        };
        #[cfg(windows)]
        let result = {
            use std::os::windows::fs::FileExt;
            file.seek_write(&buf[written..], pos + written as u64)
        };
        match result {
            Ok(0) => {
                return (written, Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0")));
            }
            Ok(n) => written += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return (written, Err(error)),
        }
    }
    (written, Ok(()))
}

/// 文件名去掉路径分隔符等危险字符。
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        "download".to_string()
    } else {
        // Windows device names remain reserved even when followed by an extension.
        let stem = trimmed.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        let numbered_device = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT")).is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        });
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || numbered_device {
            format!("_{trimmed}")
        } else {
            trimmed.to_string()
        }
    }
}

/// 以 create_new 原子占用名称，包含普通文件、目录、断链和并发任务的冲突。
pub fn reserve_file(
    dir: &Path,
    name: &str,
    registry: &crate::TargetRegistry,
) -> io::Result<(String, File, crate::TargetLease)> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 0..10_000 {
        let candidate = if n == 0 { name.to_owned() } else { format!("{stem} ({n}){ext}") };
        let lease = match registry.reserve(&dir.join(&candidate)) {
            Ok(lease) => lease,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        };
        match OpenOptions::new().create_new(true).read(true).write(true).open(dir.join(&candidate)) {
            Ok(file) => return Ok((candidate, file, lease)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "同名文件过多，请更换下载文件名"))
}

/// 续传只能打开已存在的普通文件；不创建零洞文件，也不跟随目标符号链接。
pub fn open_resume_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "续传目标不能是链接"));
        }
    }
    if !metadata.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "续传目标不是普通文件"));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_portable_to_windows_without_changing_normal_unicode_names() {
        for name in ["CON", "con.txt", "NUL.tar.gz", "COM1.txt", "lpt³.log", "AUX. "] {
            assert!(sanitize_file_name(name).starts_with('_'), "{name}");
        }
        assert_eq!(sanitize_file_name(" 报告 中文.pdf. . "), "报告 中文.pdf");
        assert_eq!(sanitize_file_name("COM10.txt"), "COM10.txt");
        assert_eq!(sanitize_file_name("../bad\\name:stream"), "_bad_name_stream");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_links_are_occupied_and_resume_does_not_follow_links() {
        let dir = tempfile::tempdir().unwrap();
        let external = dir.path().join("missing.bin");
        std::os::unix::fs::symlink(&external, dir.path().join("file.bin")).unwrap();
        let (name, _, _lease) = reserve_file(dir.path(), "file.bin", &crate::TargetRegistry::default()).unwrap();
        assert_eq!(name, "file (1).bin");
        assert!(!external.exists());
        std::fs::write(&external, b"preserve").unwrap();
        assert!(open_resume_file(&dir.path().join("file.bin")).is_err());
        assert_eq!(std::fs::read(external).unwrap(), b"preserve");
    }
}
