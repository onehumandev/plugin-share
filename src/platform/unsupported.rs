use tauri::{Runtime, Window};

use crate::models::{CanShareResult, ShareOptions};
use crate::state::PluginTempFileManager;
use crate::Error;

/// There is no native share dialog on this platform, so sharing always fails.
/// Callers should check `can_share` first.
pub fn share<R: Runtime>(
    _window: Window<R>,
    _options: ShareOptions,
    _state: &PluginTempFileManager,
) -> Result<(), Error> {
    Err(Error::NativeApi(
        "Sharing is not supported on this platform.".to_string(),
    ))
}

pub fn can_share() -> Result<CanShareResult, Error> {
    Ok(CanShareResult { value: false })
}
