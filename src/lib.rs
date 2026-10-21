//! # tauri-plugin-vnidrop-share
//!
//! A Tauri plugin to share content using the native sharing dialog on Windows, macOS, and mobile platforms.
//!
//! On desktop, the plugin handles sharing text, URLs, and files. Files passed as Base64 content are written to
//! temporary files that keep their original name, and are removed by the `cleanup` command or when the
//! application exits. Linux has no native share dialog: `can_share` returns `false` and `share` fails.
//!
//! ## Installation
//!
//! ```toml
//! [dependencies]
//! tauri-plugin-vnidrop-share = "1.0.0-rc"
//! ```
//!
//! ## Usage
//!
//! ### Rust
//!
//! You need to initialize the plugin in your `main.rs` or `lib.rs` to register the commands and set up state management.
//!
//! ```rust,ignore
//! // src/main.rs
//! fn main() {
//!     tauri::Builder::default()
//!         .plugin(tauri_plugin_vnidrop_share::init())
//!         .run(tauri::generate_context!())
//!         .expect("error while running tauri application");
//! }
//! ```
//!
//! ### Frontend (JavaScript/TypeScript)
//!
//! ```js
//! import { share, canShare } from '@vnidrop/tauri-plugin-share';
//!
//! if (await canShare()) {
//!   await share({
//!     title: 'Check this out!',
//!     text: 'I found this cool project built with Tauri.',
//!     url: 'https://tauri.app',
//!   });
//! }
//!
//! // Share a file; it is written to a temporary file and cleaned up later.
//! const file = new File(['hello'], 'document.txt', { type: 'text/plain' });
//! await share({ files: [file] });
//! ```

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

pub use models::*;

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

mod commands;
mod error;
mod models;
#[cfg(desktop)]
mod platform;
mod state;

pub use error::{Error, Result};

#[cfg(desktop)]
use desktop::Share;
#[cfg(mobile)]
use mobile::Share;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the share APIs.
pub trait ShareExt<R: Runtime> {
    fn share(&self) -> &Share<R>;
}

impl<R: Runtime, T: Manager<R>> crate::ShareExt<R> for T {
    fn share(&self) -> &Share<R> {
        self.state::<Share<R>>().inner()
    }
}

/// Initializes the plugin.
///
/// This function sets up the plugin, registers its commands, and configures the
/// state management for temporary files. The cleanup of these files is
/// automatically handled when the application exits.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("vnidrop-share")
        .invoke_handler(tauri::generate_handler![
            commands::share,
            commands::can_share,
            commands::cleanup,
        ])
        .setup(|app, api| {
            #[cfg(mobile)]
            let share = mobile::init(app, api)?;
            #[cfg(desktop)]
            let share = desktop::init(app, api)?;
            app.manage(share);
            app.manage(state::PluginTempFileManager::new());
            Ok(())
        })
        .on_drop(|app| {
            app.state::<state::PluginTempFileManager>()
                .cleanup_all_managed_files();
        })
        .build()
}
