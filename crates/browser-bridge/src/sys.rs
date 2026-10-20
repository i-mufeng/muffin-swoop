//! 平台分发：Windows 走注册表 / 文件系统实现，其他平台提供空实现（CI 在 macOS 上构建）。

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub(crate) use stub::*;
