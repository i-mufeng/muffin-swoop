//! 非 Windows 平台：浏览器集成是 Windows 专属能力，这里全部返回空实现。

use crate::install::{ExtensionInfo, ExtensionStatus};
use crate::{BridgeError, DetectedBrowser, InstallOutcome};

pub(crate) fn detect_installed(_defs: &[crate::BrowserDef]) -> Vec<DetectedBrowser> {
    Vec::new()
}

pub(crate) fn install_extension(_b: &DetectedBrowser, _e: &ExtensionInfo) -> Result<InstallOutcome, BridgeError> {
    Err(BridgeError::Unsupported("浏览器扩展集成仅支持 Windows".into()))
}

pub(crate) fn uninstall_extension(_b: &DetectedBrowser, _e: &ExtensionInfo) -> Result<(), BridgeError> {
    Err(BridgeError::Unsupported("浏览器扩展集成仅支持 Windows".into()))
}

pub(crate) fn guide(_b: &DetectedBrowser, _e: &ExtensionInfo) -> Result<InstallOutcome, BridgeError> {
    Err(BridgeError::Unsupported("浏览器扩展集成仅支持 Windows".into()))
}

pub(crate) fn status(_b: &DetectedBrowser, _e: &ExtensionInfo) -> ExtensionStatus {
    ExtensionStatus::Unknown
}
