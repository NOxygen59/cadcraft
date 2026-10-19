//! iPadOS: every file dialog is the Files app's document picker, presented over the window.
//!
//! - Open picks a file where it is (iCloud Drive, On My iPad, a file provider), not a copy:
//!   Save writes back to the same file. Access to each file picked is kept for the session.
//! - Save picks the folder to save in; the file keeps the name the app suggests.
//!
//! The app's own Documents folder is where the picker starts; with `UIFileSharingEnabled` and
//! `LSSupportsOpeningDocumentsInPlace` (Info.plist) it shows in Files as On My iPad › CADCraft.
//!
//! UIKit's delegate protocol, the window's raw view and the resource counters have no safe API,
//! hence the scoped `unsafe_code` allowance.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use cadcraft_ui_egui::ResourceReading;
use cadcraft_ui_egui::picks::PickRequest;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSArray, NSString, NSURL};
use objc2_ui_kit::{UIDocumentPickerDelegate, UIDocumentPickerViewController, UIView};
use objc2_uniform_type_identifiers::{UTType, UTTypeFolder, UTTypeItem};
use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

/// The app's Documents folder (shown in Files), made if missing.
pub fn documents_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?).join("Documents");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

struct Ivars {
    answer: RefCell<Option<Sender<Option<String>>>>,
    ui: egui::Context,
    /// Save: the file name, joined to the folder picked.
    save_as: Option<String>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CADCraftDocumentPickerDelegate"]
    #[ivars = Ivars]
    struct PickerDelegate;

    unsafe impl NSObjectProtocol for PickerDelegate {}

    unsafe impl UIDocumentPickerDelegate for PickerDelegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn did_pick(&self, _picker: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            let path = urls.iter().find_map(|url| {
                // Kept for the session: the app reads the file, and writes it on Save, later.
                let _ = unsafe { url.startAccessingSecurityScopedResource() };
                Some(url.path()?.to_string())
            });
            self.answer(path);
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn cancelled(&self, _picker: &UIDocumentPickerViewController) {
            self.answer(None);
        }
    }
);

impl PickerDelegate {
    fn new(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    fn answer(&self, path: Option<String>) {
        let ivars = self.ivars();
        let path = match &ivars.save_as {
            Some(name) => path.map(|folder| Path::new(&folder).join(name).to_string_lossy().into_owned()),
            None => path,
        };
        if let Some(tx) = ivars.answer.borrow_mut().take() {
            // The app gone meanwhile has no use for the answer.
            let _ = tx.send(path);
        }
        ivars.ui.request_repaint();
    }
}

thread_local! {
    /// The picker's delegate (the picker holds it weakly), until the next picker replaces it.
    static DELEGATE: RefCell<Option<Retained<PickerDelegate>>> = const { RefCell::new(None) };
}

/// Show the document picker for `request` over `window`, answering on the receiver.
pub fn start_pick(request: PickRequest, window: Option<&Arc<winit::window::Window>>, ui: &egui::Context) -> Option<Receiver<Option<String>>> {
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("no document picker for {request:?}: not on the main thread");
        return None;
    };
    let RawWindowHandle::UiKit(handle) = window?.window_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: winit's UIKit handle is the window's UIView, alive as long as the window is.
    let view: &UIView = unsafe { handle.ui_view.cast::<UIView>().as_ref() };
    let mut presenter = view.window()?.rootViewController()?;
    while let Some(next) = presenter.presentedViewController() {
        presenter = next;
    }

    let (folder, save_as) = match request {
        PickRequest::Open => (false, None),
        PickRequest::Save(name) => (true, Some(if name.is_empty() { "Drawing.dwg".to_string() } else { name })),
    };
    let kind: &UTType = unsafe { if folder { UTTypeFolder } else { UTTypeItem } };
    let types = NSArray::from_slice(&[kind]);
    let picker = UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(UIDocumentPickerViewController::alloc(mtm), &types, false);
    picker.setAllowsMultipleSelection(false);
    picker.setShouldShowFileExtensions(true);
    if let Some(start) = documents_dir() {
        picker.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(&start.to_string_lossy()))));
    }

    let (tx, rx) = channel();
    let delegate = PickerDelegate::new(mtm, Ivars { answer: RefCell::new(Some(tx)), ui: ui.clone(), save_as });
    picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
    presenter.presentViewController_animated_completion(&picker, true, None);
    Some(rx)
}

// libSystem has it on iOS too; the `libc` crate declares it for macOS only.
unsafe extern "C" {
    fn proc_pid_rusage(pid: libc::c_int, flavor: libc::c_int, buffer: *mut libc::rusage_info_t) -> libc::c_int;
}

/// The memory and CPU CADCraft uses, and the battery level, for the menu bar's readout. Cheap
/// (two system calls and two UIDevice properties); the CPU share is measured since the previous
/// call. The battery needs the main thread (unknown off it).
pub fn resources() -> ResourceReading {
    use std::sync::Mutex;
    use std::time::Instant;
    static LAST: Mutex<Option<(Instant, f64)>> = Mutex::new(None);

    let mut out = ResourceReading::default();
    // The footprint is what Xcode's memory gauge and iPadOS's memory limit use.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    if unsafe { proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V2, (&raw mut info).cast()) } == 0 {
        out.memory_bytes = info.ri_phys_footprint;
    }
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) } == 0 {
        let secs = |t: libc::timeval| t.tv_sec as f64 + f64::from(t.tv_usec) / 1e6;
        let cpu = secs(usage.ru_utime) + secs(usage.ru_stime);
        let now = Instant::now();
        let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((then, before)) = *last {
            let wall = now.duration_since(then).as_secs_f64();
            let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get) as f64;
            if wall > 0.0 {
                out.cpu_percent = ((cpu - before) / wall / cores * 100.0).clamp(0.0, 100.0) as f32;
            }
        }
        *last = Some((now, cpu));
    }
    if let Some(mtm) = MainThreadMarker::new() {
        let device = objc2_ui_kit::UIDevice::currentDevice(mtm);
        if !device.isBatteryMonitoringEnabled() {
            device.setBatteryMonitoringEnabled(true);
        }
        let level = device.batteryLevel();
        if level >= 0.0 {
            out.battery_percent = Some(level * 100.0);
        }
        let state = device.batteryState();
        out.charging = state == objc2_ui_kit::UIDeviceBatteryState::Charging || state == objc2_ui_kit::UIDeviceBatteryState::Full;
    }
    out
}

/// Where the 60 / 120 Hz choice is kept (the app doesn't persist egui's memory).
fn frame_rate_file() -> Option<PathBuf> {
    crate::config_dir().map(|d| d.join("max_frame_rate"))
}

/// Cap redraws at `fps` and remember the choice.
pub fn set_max_frame_rate(fps: u32) {
    winit::platform::ios::set_max_frame_rate(fps);
    if let Some(file) = frame_rate_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&file, fps.to_string()) {
            log::warn!("frame rate choice not saved: {e}");
        }
    }
}

/// The cap chosen last time: 60 or 120 (60 when never chosen).
pub fn saved_max_frame_rate() -> u32 {
    let saved = frame_rate_file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| s.trim().parse::<u32>().ok());
    if saved == Some(120) { 120 } else { 60 }
}
