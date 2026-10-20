#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use self::windows::*;

#[cfg(target_os = "windows")]
mod focus;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod temp_files;

#[cfg(all(desktop, not(any(target_os = "windows", target_os = "macos"))))]
mod unsupported;
#[cfg(all(desktop, not(any(target_os = "windows", target_os = "macos"))))]
pub use self::unsupported::*;
