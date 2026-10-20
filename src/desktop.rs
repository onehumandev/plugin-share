use crate::state::PluginTempFileManager;
use crate::{models::*, Result};
use tauri::plugin::PluginApi;
use tauri::{AppHandle, Manager, Runtime, State, Window};

use crate::platform;

/// A handle to the `tauri-plugin-share` APIs for desktop.
///
/// This struct provides the public interface for the plugin's commands,
/// abstracting away the platform-specific implementations.
pub struct Share<R: Runtime>(AppHandle<R>);

impl<R: Runtime> Share<R> {
    /// Shows the native share dialog and blocks until it is dismissed.
    ///
    /// Call this from a blocking context (for example `spawn_blocking`), not
    /// directly on an async runtime worker or the main thread.
    pub fn share(
        &self,
        window: Window<R>,
        options: ShareOptions,
        state: State<'_, PluginTempFileManager>,
    ) -> Result<()> {
        let options = prepare_share_options(options)?;
        let _active_share = state.begin_share();
        platform::share(window, options, state.inner())
    }

    pub fn can_share(&self) -> Result<CanShareResult> {
        platform::can_share()
    }

    pub fn cleanup(&self) -> Result<()> {
        let state = self.0.state::<PluginTempFileManager>();
        if state.is_sharing() {
            return Err(crate::Error::ShareInProgress);
        }
        state.cleanup_all_managed_files();
        Ok(())
    }
}

pub fn init<R: Runtime, C: serde::de::DeserializeOwned>(
    app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> crate::Result<Share<R>> {
    Ok(Share(app.clone()))
}
