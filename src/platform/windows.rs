use super::focus;
use super::temp_files::stage_files;
use crate::state::PluginTempFileManager;
use crate::{CanShareResult, Error, ShareOptions};
use log::error;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicIsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tauri::{Runtime, Window};
use windows::core::{AgileReference, Interface, HSTRING};
use windows::ApplicationModel::DataTransfer::{
    DataPackageOperation, DataRequestedEventArgs, DataTransferManager,
};
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Storage::{IStorageItem, StorageFile};
use windows::Win32::{
    Foundation::HWND,
    System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_SINGLETHREADED},
    UI::Shell::IDataTransferManagerInterop,
};
use windows_collections::IIterable;

const DATA_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// Holds each window's DataTransferManager and its DataRequested registration token,
// keyed by HWND, for the duration of the share. It is only touched on the main
// thread, which is where these WinRT objects live.
thread_local! {
    static SHARE_REGISTRATIONS: RefCell<HashMap<isize, (DataTransferManager, i64)>> =
        RefCell::new(HashMap::new());
}

// A helper to map the detailed windows::core::Error into our plugin's simpler error type.
impl From<windows::core::Error> for Error {
    fn from(err: windows::core::Error) -> Self {
        Error::NativeApi(err.message().to_string())
    }
}

/// Everything the DataRequested handler puts into the data package. Files are
/// resolved to storage items before the share UI is shown; agile references let
/// them move from the calling thread to the UI thread.
struct ShareContent {
    title: String,
    text: Option<String>,
    url: Option<String>,
    storage_items: Vec<AgileReference<IStorageItem>>,
}

pub fn can_share() -> Result<CanShareResult, Error> {
    Ok(CanShareResult { value: true })
}

pub fn share<R: Runtime>(
    window: Window<R>,
    options: ShareOptions,
    state: &PluginTempFileManager,
) -> Result<(), Error> {
    let content = prepare_content(&options, state)?;
    let focus_wait = focus::begin_focus_wait(&window)?;
    let (tx, rx) = mpsc::channel();
    let registered_hwnd = Arc::new(AtomicIsize::new(0));

    let main_window = window.clone();
    let main_registered_hwnd = registered_hwnd.clone();
    if let Err(e) = window.run_on_main_thread(move || {
        if let Err(err) = show_share_ui(&main_window, content, tx.clone(), &main_registered_hwnd) {
            let _ = tx.send(Err(err));
        }
    }) {
        focus_wait.cancel();
        return Err(e.into());
    }

    match rx.recv_timeout(DATA_REQUEST_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            focus_wait.cancel();
            return Err(err);
        }
        Err(err) => {
            focus_wait.cancel();
            let hwnd_key = registered_hwnd.load(Ordering::SeqCst);
            if hwnd_key != 0 {
                let _ = window.run_on_main_thread(move || unregister(hwnd_key, None));
            }
            return Err(Error::NativeApi(format!(
                "Share timed out waiting for DataRequested event: {}",
                err
            )));
        }
    }

    // The data has been handed to the share UI; now wait for the window to lose
    // and regain focus, which is how desktop apps observe the dialog closing.
    focus_wait.start_grace_timer();
    focus_wait.wait()
}

/// Writes Base64 files to disk and resolves every file to a WinRT storage item.
/// Runs on the calling thread so missing files fail before any UI is shown and
/// the UI thread never blocks on file system calls.
fn prepare_content(
    options: &ShareOptions,
    state: &PluginTempFileManager,
) -> Result<ShareContent, Error> {
    let temp_paths = stage_files(options.files.as_deref(), state)?;
    // WinRT's StorageFile API is stricter about separators than the frontend, so
    // normalize forward slashes in caller-provided paths.
    let file_paths = options
        .file_paths
        .iter()
        .flatten()
        .map(|path| PathBuf::from(path.replace('/', "\\")));

    let storage_items = temp_paths
        .into_iter()
        .chain(file_paths)
        .map(|path| storage_item_for_path(&path))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ShareContent {
        title: share_title(options),
        text: options.text.clone(),
        url: options.url.clone(),
        storage_items,
    })
}

/// Windows requires a non-empty title. Use the caller's title, else the first
/// file name, else a generic label.
fn share_title(options: &ShareOptions) -> String {
    let first_path_name = options
        .file_paths
        .iter()
        .flatten()
        .next()
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let first_file_name = options
        .files
        .iter()
        .flatten()
        .next()
        .map(|file| file.name.clone());
    options
        .title
        .clone()
        .or(first_path_name)
        .or(first_file_name)
        .unwrap_or_else(|| "Shared content".to_string())
}

fn storage_item_for_path(path: &Path) -> Result<AgileReference<IStorageItem>, Error> {
    let display = path.display();
    let storage_file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))
        .and_then(|operation| operation.get())
        .map_err(|e| {
            Error::NativeApi(format!("Failed to open '{}' for sharing: {}", display, e))
        })?;
    storage_file
        .cast::<IStorageItem>()
        .and_then(|item| AgileReference::new(&item))
        .map_err(|e| {
            Error::NativeApi(format!(
                "Failed to prepare '{}' for sharing: {}",
                display, e
            ))
        })
}

/// Registers the DataRequested handler for the window and shows the share UI.
/// Must run on the main thread.
fn show_share_ui<R: Runtime>(
    window: &Window<R>,
    content: ShareContent,
    tx: mpsc::Sender<Result<(), Error>>,
    registered_hwnd: &AtomicIsize,
) -> Result<(), Error> {
    let _winrt = WinRtGuard::initialize()?;
    let hwnd = get_hwnd(window)?;
    let hwnd_key = hwnd.0 as isize;
    let (dtm, interop) = get_data_transfer_manager(hwnd)?;

    // A previous share on this window that never received DataRequested would
    // otherwise keep its handler and add its data to this share.
    unregister(hwnd_key, None);

    let token_cell = Arc::new(AtomicI64::new(0));
    let handler_token = token_cell.clone();
    let handler = TypedEventHandler::new(
        move |_,
              args: windows::core::Ref<'_, DataRequestedEventArgs>|
              -> windows::core::Result<()> {
            let result = (*args)
                .as_ref()
                .ok_or_else(|| Error::NativeApi("Missing DataRequestedEventArgs".to_string()))
                .and_then(|args| fill_data_package(args, &content));
            if let Err(e) = &result {
                error!("[share] DataRequested handler failed: {}", e);
            }
            let _ = tx.send(result);
            unregister(hwnd_key, Some(handler_token.load(Ordering::SeqCst)));
            Ok(())
        },
    );

    let token = dtm.DataRequested(&handler)?;
    token_cell.store(token, Ordering::SeqCst);
    SHARE_REGISTRATIONS.with(|registrations| {
        registrations.borrow_mut().insert(hwnd_key, (dtm, token));
    });
    registered_hwnd.store(hwnd_key, Ordering::SeqCst);

    if let Err(e) = unsafe { interop.ShowShareUIForWindow(hwnd) } {
        unregister(hwnd_key, Some(token));
        return Err(e.into());
    }
    Ok(())
}

/// Puts the prepared content into the share request's data package.
fn fill_data_package(args: &DataRequestedEventArgs, content: &ShareContent) -> Result<(), Error> {
    let data = args.Request()?.Data()?;
    data.Properties()?
        .SetTitle(&HSTRING::from(&content.title))?;

    let web_link =
        content
            .url
            .as_deref()
            .and_then(|url| match Uri::CreateUri(&HSTRING::from(url)) {
                Ok(uri) => Some(uri),
                Err(_) => {
                    error!(
                        "Could not parse URL '{}' for DataPackage::SetWebLink; sharing it as text.",
                        url
                    );
                    None
                }
            });

    // Targets that only read text should still see the URL, so it is appended to
    // the text unless it could be set as a proper web link.
    let text = match (&content.text, &content.url, &web_link) {
        (Some(text), _, Some(_)) => Some(text.clone()),
        (Some(text), Some(url), None) => Some(format!("{text}\n{url}")),
        (Some(text), None, None) => Some(text.clone()),
        (None, Some(url), None) => Some(url.clone()),
        (None, _, _) => None,
    };
    if let Some(text) = text {
        data.SetText(&HSTRING::from(text))?;
    }
    if let Some(uri) = web_link {
        data.SetWebLink(&uri)?;
    }

    if !content.storage_items.is_empty() {
        let items = content
            .storage_items
            .iter()
            .map(|item| item.resolve().map(Some))
            .collect::<windows::core::Result<Vec<Option<IStorageItem>>>>()?;
        let iterable_items: IIterable<IStorageItem> = items.into();
        data.SetRequestedOperation(DataPackageOperation::Copy)
            .map_err(|e| Error::NativeApi(format!("Failed to set requested operation: {}", e)))?;
        data.SetStorageItems(&iterable_items, true).map_err(|e| {
            Error::NativeApi(format!("Failed to set storage items on DataPackage: {}", e))
        })?;
    }

    Ok(())
}

/// Removes the window's DataRequested registration. With `Some(token)`, only that
/// registration is removed, so a finished share never removes a newer one.
fn unregister(hwnd_key: isize, token: Option<i64>) {
    SHARE_REGISTRATIONS.with(|registrations| {
        let mut registrations = registrations.borrow_mut();
        let matches = registrations
            .get(&hwnd_key)
            .is_some_and(|(_, registered)| token.is_none_or(|token| token == *registered));
        if matches {
            if let Some((manager, registered)) = registrations.remove(&hwnd_key) {
                let _ = manager.RemoveDataRequested(registered);
            }
        }
    });
}

/// Initializes the Windows Runtime on the current thread for the guard's lifetime.
/// Each successful `RoInitialize` (including `S_FALSE` when it was already
/// initialized) is balanced with `RoUninitialize`.
struct WinRtGuard;

impl WinRtGuard {
    fn initialize() -> Result<Self, Error> {
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }
            .map_err(|e| Error::NativeApi(format!("Failed to initialize WinRT: {}", e)))?;
        Ok(Self)
    }
}

impl Drop for WinRtGuard {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

/// Retrieves the native window handle (HWND) from the Tauri window.
fn get_hwnd<R: Runtime>(window: &Window<R>) -> Result<HWND, Error> {
    let handle = window
        .window_handle()
        .map_err(|e| Error::NativeApi(e.to_string()))?;

    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(HWND(handle.hwnd.get() as *mut std::ffi::c_void)),
        _ => Err(Error::NativeApi(
            "Unsupported window handle type".to_string(),
        )),
    }
}

/// Gets an instance of the DataTransferManager associated with the window's HWND.
/// This is the required method for desktop (non-UWP) applications. [1]
fn get_data_transfer_manager(
    hwnd: HWND,
) -> Result<(DataTransferManager, IDataTransferManagerInterop), Error> {
    let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()?;
    let dtm = unsafe { interop.GetForWindow(hwnd) }?;
    Ok((dtm, interop))
}
