//! 按扩展名推断文件类型，决定卡片左侧的图标与色块颜色。

use egui::Color32;
use egui_phosphor::fill as i;

use crate::theme::Palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Archive,
    Disk,
    Pdf,
    Doc,
    Sheet,
    Slides,
    Video,
    Audio,
    Image,
    Code,
    Package,
    Text,
    Other,
}

impl FileKind {
    pub fn from_name(name: &str) -> Self {
        let ext = name.rsplit('.').next().map(|e| e.to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "xip" => Self::Archive,
            "iso" | "img" | "dmg" | "vhd" | "vmdk" => Self::Disk,
            "pdf" => Self::Pdf,
            "doc" | "docx" | "odt" | "rtf" | "pages" => Self::Doc,
            "xls" | "xlsx" | "csv" | "ods" | "numbers" => Self::Sheet,
            "ppt" | "pptx" | "key" | "odp" => Self::Slides,
            "mp4" | "mkv" | "mov" | "avi" | "webm" | "flv" | "m4v" | "ts" => Self::Video,
            "mp3" | "flac" | "wav" | "aac" | "m4a" | "ogg" | "opus" => Self::Audio,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "heic" | "avif" | "bmp" | "psd" => Self::Image,
            "rs" | "js" | "tsx" | "py" | "go" | "java" | "c" | "cpp" | "h" | "json" | "yaml" | "yml" | "toml" | "sh" => {
                Self::Code
            }
            "pkg" | "exe" | "msi" | "deb" | "rpm" | "apk" | "appimage" | "app" => Self::Package,
            "txt" | "md" | "log" | "srt" => Self::Text,
            _ => Self::Other,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Archive => i::FILE_ZIP,
            Self::Disk => i::DISC,
            Self::Pdf => i::FILE_PDF,
            Self::Doc => i::FILE_DOC,
            Self::Sheet => i::FILE_XLS,
            Self::Slides => i::FILE_PPT,
            Self::Video => i::FILE_VIDEO,
            Self::Audio => i::FILE_AUDIO,
            Self::Image => i::FILE_IMAGE,
            Self::Code => i::FILE_CODE,
            Self::Package => i::PACKAGE,
            Self::Text => i::FILE_TEXT,
            Self::Other => i::FILE,
        }
    }

    /// 线框图标（侧栏等小尺寸处）
    pub fn outline_icon(self) -> &'static str {
        use egui_phosphor::regular as o;
        match self {
            Self::Archive => o::FILE_ZIP,
            Self::Disk => o::DISC,
            Self::Pdf => o::FILE_PDF,
            Self::Doc => o::FILE_DOC,
            Self::Sheet => o::FILE_XLS,
            Self::Slides => o::FILE_PPT,
            Self::Video => o::FILE_VIDEO,
            Self::Audio => o::FILE_AUDIO,
            Self::Image => o::FILE_IMAGE,
            Self::Code => o::FILE_CODE,
            Self::Package => o::PACKAGE,
            Self::Text => o::FILE_TEXT,
            Self::Other => o::FOLDER_SIMPLE,
        }
    }

    pub fn tint(self, p: &Palette) -> Color32 {
        match self {
            Self::Archive => p.tint_amber,
            Self::Disk => p.tint_cyan,
            Self::Pdf => p.tint_red,
            Self::Doc => p.tint_blue,
            Self::Sheet => p.tint_green,
            Self::Slides => p.tint_amber,
            Self::Video => p.tint_purple,
            Self::Audio => p.tint_pink,
            Self::Image => p.tint_green,
            Self::Code => p.tint_cyan,
            Self::Package => p.tint_blue,
            Self::Text => p.text_muted,
            Self::Other => p.text_muted,
        }
    }
}
