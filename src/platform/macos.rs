use super::temp_files::stage_files;
use crate::models::CanShareResult;
use crate::state::PluginTempFileManager;
use crate::{Error, ShareOptions};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::Message;
use objc2::{
    define_class, msg_send,
    rc::{autoreleasepool, Retained},
    AnyThread, DefinedClass, MainThreadOnly,
};
use objc2_app_kit::{
    NSSharingService, NSSharingServiceDelegate, NSSharingServicePicker,
    NSSharingServicePickerDelegate, NSView,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSError, NSObject, NSObjectProtocol, NSString, NSURL,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle, WindowHandle};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use tauri::{Runtime, Window};

const SHARE_COMPLETION_TIMEOUT: Duration = Duration::from_secs(60);

thread_local! {
    static ACTIVE_DELEGATES: RefCell<Vec<Retained<SharePickerDelegate>>> = const { RefCell::new(Vec::new()) };
}

#[derive(Default)]
struct ShareDelegateIvars {
    completion: RefCell<Option<mpsc::Sender<Result<(), Error>>>>,
    subject: RefCell<Option<Retained<NSString>>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ShareDelegateIvars]
    struct SharePickerDelegate;

    unsafe impl NSObjectProtocol for SharePickerDelegate {}

    unsafe impl NSSharingServicePickerDelegate for SharePickerDelegate {
        #[unsafe(method_id(sharingServicePicker:delegateForSharingService:))]
        fn sharing_service_picker_delegate_for_sharing_service(
            &self,
            _picker: &NSSharingServicePicker,
            service: &NSSharingService,
        ) -> Option<Retained<ProtocolObject<dyn NSSharingServiceDelegate>>> {
            if let Some(subject) = self.ivars().subject.borrow().as_ref() {
                service.setSubject(Some(&**subject));
            }
            Some(ProtocolObject::from_retained(self.retain()))
        }

        #[unsafe(method(sharingServicePicker:didChooseSharingService:))]
        fn sharing_service_picker_did_choose_sharing_service(
            &self,
            _picker: &NSSharingServicePicker,
            service: Option<&NSSharingService>,
        ) {
            if service.is_none() {
                self.complete(Ok(()));
            }
        }
    }

    unsafe impl NSSharingServiceDelegate for SharePickerDelegate {
        #[unsafe(method(sharingService:didShareItems:))]
        fn sharing_service_did_share_items(&self, _service: &NSSharingService, _items: &NSArray) {
            self.complete(Ok(()));
        }

        #[unsafe(method(sharingService:didFailToShareItems:error:))]
        fn sharing_service_did_fail_to_share_items_error(
            &self,
            _service: &NSSharingService,
            _items: &NSArray,
            error: &NSError,
        ) {
            let message = autoreleasepool(|pool| unsafe {
                error.localizedDescription().to_str(pool).to_string()
            });
            self.complete(Err(Error::NativeApi(format!(
                "Sharing failed: {}",
                message
            ))));
        }
    }
);

impl SharePickerDelegate {
    fn new(
        mtm: MainThreadMarker,
        completion: mpsc::Sender<Result<(), Error>>,
        subject: Option<Retained<NSString>>,
    ) -> Retained<Self> {
        let ivars = ShareDelegateIvars {
            completion: RefCell::new(Some(completion)),
            subject: RefCell::new(subject),
        };
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    fn complete(&self, result: Result<(), Error>) {
        if let Some(tx) = self.ivars().completion.borrow_mut().take() {
            let _ = tx.send(result);
        }
        remove_active_delegate(self);
    }
}

fn remove_active_delegate(delegate: &SharePickerDelegate) {
    let delegate_ptr = delegate as *const SharePickerDelegate;
    ACTIVE_DELEGATES.with(|delegates| {
        let mut list = delegates.borrow_mut();
        if let Some(pos) = list
            .iter()
            .position(|item| Retained::as_ptr(item) == delegate_ptr)
        {
            list.remove(pos);
        }
    });
}

pub fn can_share() -> Result<CanShareResult, Error> {
    // On macOS, we can always share as long as the sharing service is available.
    Ok(CanShareResult { value: true })
}

/// Shares content using the native macOS sharing service.
///
/// Resolves when a service reports success, when the picker is dismissed
/// without choosing a service, or after `SHARE_COMPLETION_TIMEOUT` if the
/// chosen service never reports back.
pub fn share<R: Runtime>(
    window: Window<R>,
    options: ShareOptions,
    state: &PluginTempFileManager,
) -> Result<(), Error> {
    // Decode and write files here, on the calling thread, so the main thread
    // only builds and shows the picker.
    let temp_paths = stage_files(options.files.as_deref(), state)?;

    let (setup_tx, setup_rx) = mpsc::channel();
    let (completion_tx, completion_rx) = mpsc::channel();
    let window_clone = window.clone();

    window.run_on_main_thread(move || {
        let result = show_share_picker(&window_clone, &options, &temp_paths, completion_tx);
        let _ = setup_tx.send(result);
    })?;

    setup_rx.recv()??;

    match completion_rx.recv_timeout(SHARE_COMPLETION_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
        Err(mpsc::RecvTimeoutError::Disconnected) => Ok(()),
    }
}

/// Builds the shared items and shows the picker. Must run on the main thread.
fn show_share_picker<R: Runtime>(
    window: &Window<R>,
    options: &ShareOptions,
    temp_paths: &[PathBuf],
    completion_tx: mpsc::Sender<Result<(), Error>>,
) -> Result<(), Error> {
    let mtm = MainThreadMarker::new().ok_or_else(|| {
        Error::NativeApi("The share picker must be shown on the main thread.".to_string())
    })?;
    let ns_view = get_ns_view(window)?;
    let mut items_to_share: Vec<Retained<NSObject>> = Vec::new();

    if let Some(url) = options.url.as_deref() {
        if let Some(url_obj) = NSURL::URLWithString(&NSString::from_str(url)) {
            items_to_share.push(unsafe { Retained::cast_unchecked(url_obj) });
        }
    }

    if let Some(text) = options.text.as_deref() {
        items_to_share.push(unsafe { Retained::cast_unchecked(NSString::from_str(text)) });
    }

    // Temporary files written from Base64 data, then local files shared
    // directly from their original paths.
    let file_paths = options.file_paths.iter().flatten().map(PathBuf::from);
    for path in temp_paths.iter().cloned().chain(file_paths) {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        items_to_share.push(unsafe { Retained::cast_unchecked(url) });
    }

    if items_to_share.is_empty() {
        return Err(Error::InvalidArgs(
            "No content provided to share.".to_string(),
        ));
    }

    let subject = options.title.as_deref().map(NSString::from_str);

    autoreleasepool(|_pool| {
        let objects_refs: Vec<&AnyObject> = items_to_share
            .iter()
            .map(|obj| obj.as_ref() as &AnyObject)
            .collect();
        let items_array = NSArray::from_slice(&objects_refs);
        let picker = unsafe {
            NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items_array)
        };

        let delegate = SharePickerDelegate::new(mtm, completion_tx, subject);
        ACTIVE_DELEGATES.with(|delegates| delegates.borrow_mut().push(delegate.retain()));
        picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let bounds = ns_view.bounds();
        // Web-viewport coordinates are top-left origin; AppKit bounds use a bottom-left origin.
        // Flip the y-axis so the anchor rect describes the same visual area in NSView space.
        // Without an anchor, use a 1x1 rect at the view's centre. A zero-size rect
        // makes AppKit fall back to the whole view's bounds, which pushes the picker
        // to the window's edge.
        let rect = options.anchor.map_or(
            CGRect {
                origin: CGPoint {
                    x: bounds.size.width / 2.0,
                    y: bounds.size.height / 2.0,
                },
                size: CGSize {
                    width: 1.0,
                    height: 1.0,
                },
            },
            |anchor| CGRect {
                origin: CGPoint {
                    x: anchor.x,
                    y: bounds.size.height - anchor.y - anchor.height,
                },
                size: CGSize {
                    width: anchor.width,
                    height: anchor.height,
                },
            },
        );
        picker.showRelativeToRect_ofView_preferredEdge(
            rect,
            &ns_view,
            objc2_foundation::NSRectEdge::NSMinYEdge,
        );
    });
    Ok(())
}

/// Retrieves the native `NSView` pointer from the Tauri window, compatible with `raw-window-handle`.
fn get_ns_view<R: Runtime>(window: &Window<R>) -> Result<Retained<NSView>, Error> {
    let window_handle: WindowHandle<'_> = window.window_handle()?;
    if let RawWindowHandle::AppKit(handle) = window_handle.as_raw() {
        let ns_view_ptr = handle.ns_view.as_ptr();
        let ns_view: Retained<NSView> = unsafe { Retained::retain(ns_view_ptr.cast()) }.unwrap();
        Ok(ns_view)
    } else {
        Err(Error::NativeApi(
            "Unsupported window handle type on macOS.".to_string(),
        ))
    }
}
