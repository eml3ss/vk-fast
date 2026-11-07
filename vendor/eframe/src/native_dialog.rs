//! Native ownership for dialogs hosted in a separate application process.
use std::sync::Arc;

#[cfg(target_os = "linux")]
use raw_window_handle::{HasDisplayHandle as _, RawDisplayHandle};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use winit::window::Window;

/// A native root retained while an external dialog establishes ownership.
#[derive(Clone)]
pub struct NativeDialogParent {
    window: Arc<Window>,
}

/// Read readiness for a private helper pipe without blocking on inherited writers.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub fn windows_pipe_available(
    handle: std::os::windows::io::BorrowedHandle<'_>,
) -> std::io::Result<u32> {
    use std::os::windows::io::AsRawHandle as _;
    let mut available = 0;
    // The borrowed read handle remains owned by the only consuming thread.
    let result = unsafe {
        windows_sys::Win32::System::Pipes::PeekNamedPipe(
            handle.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(available)
    }
}

/// An exported dialog parent. Keep alive until the owned dialog has closed.
pub struct NativeDialogParentHandle {
    #[cfg(target_os = "linux")]
    _wayland: Option<wayland::Export>,
    _window: Arc<Window>,
    identifier: String,
}

impl NativeDialogParentHandle {
    /// A process-local or compositor-issued identifier for the native parent.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }
}

impl NativeDialogParent {
    pub(crate) fn new(window: Arc<Window>) -> Self {
        Self { window }
    }

    /// Export off the UI thread. Keep the result alive until the dialog closes.
    pub fn export(&self) -> Option<NativeDialogParentHandle> {
        let raw = self.window.window_handle().ok()?.as_raw();
        #[cfg(target_os = "linux")]
        let mut wayland = None;
        let identifier = match raw {
            RawWindowHandle::Win32(handle) => format!("win32:{:x}", handle.hwnd.get()),
            RawWindowHandle::Xlib(handle) => format!("x11:{:x}", handle.window),
            RawWindowHandle::Xcb(handle) => format!("x11:{:x}", handle.window.get()),
            #[cfg(target_os = "linux")]
            RawWindowHandle::Wayland(surface) => {
                let RawDisplayHandle::Wayland(display) =
                    self.window.display_handle().ok()?.as_raw()
                else {
                    return None;
                };
                let exported = wayland::Export::new(surface, display)?;
                let identifier = format!("wayland:{}", exported.identifier);
                wayland = Some(exported);
                identifier
            }
            _ => return None,
        };
        Some(NativeDialogParentHandle {
            #[cfg(target_os = "linux")]
            _wayland: wayland,
            _window: self.window.clone(),
            identifier,
        })
    }
}

#[cfg(target_os = "linux")]
/// Attach a GTK dialog to an exported Wayland or X11 application window.
pub fn attach_gtk_dialog(window: &impl gtk::prelude::IsA<gtk::Window>, identifier: &str) -> bool {
    use gtk::{
        glib::{object::Cast as _, translate::ToGlibPtr as _},
        prelude::*,
    };
    let window = window.as_ref();
    window.realize();
    window.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
    let Some(surface) = window.window() else {
        return false;
    };
    let attached = if let Some(handle) = identifier.strip_prefix("wayland:") {
        if surface.type_().name() != "GdkWaylandWindow" || handle.is_empty() || handle.len() > 512 {
            return false;
        }
        let Ok(handle) = std::ffi::CString::new(handle) else {
            return false;
        };
        let raw: *mut gtk::gdk::ffi::GdkWindow = surface.to_glib_none().0;
        // GTK owns this realized surface, whose Wayland subtype was checked
        // above. The borrow stays alive throughout this main-thread call.
        #[allow(unsafe_code)]
        unsafe {
            gdk_wayland_sys::gdk_wayland_window_set_transient_for_exported(
                raw.cast(),
                handle.as_ptr(),
            ) != 0
        }
    } else if let Some(xid) = identifier
        .strip_prefix("x11:")
        .and_then(|id| u32::from_str_radix(id, 16).ok())
        .filter(|id| *id != 0)
    {
        let Ok(display) = surface.display().downcast::<gdkx11::X11Display>() else {
            return false;
        };
        let parent = gdkx11::X11Window::foreign_new_for_display(&display, xid.into());
        surface.set_transient_for(parent.upcast_ref());
        true
    } else {
        false
    };
    if attached {
        window.set_modal(true);
    }
    attached
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
/// Give a separately hosted dialog an owner from its launching process.
pub fn attach_windows_dialog(child: isize, identifier: &str, expected_parent: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::SetLastError,
        UI::WindowsAndMessaging::{
            GWLP_HWNDPARENT, GetWindowThreadProcessId, IsWindow, SetWindowLongPtrW,
        },
    };
    let Some(owner) = identifier
        .strip_prefix("win32:")
        .and_then(|id| isize::from_str_radix(id, 16).ok())
        .filter(|id| *id != 0)
    else {
        return false;
    };
    let child = child as *mut std::ffi::c_void;
    let owner = owner as *mut std::ffi::c_void;
    let mut process = 0;
    // HWNDs are checked without dereferencing their opaque values. Only the
    // dialog is changed, and the owner must belong to its launching process.
    unsafe {
        if IsWindow(child) == 0
            || IsWindow(owner) == 0
            || GetWindowThreadProcessId(owner, &mut process) == 0
            || process != expected_parent
        {
            return false;
        }
        SetLastError(0);
        let previous = SetWindowLongPtrW(child, GWLP_HWNDPARENT, owner as isize);
        previous != 0 || windows_sys::Win32::Foundation::GetLastError() == 0
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
/// Center a native dialog over its current owner without activating either one.
pub fn center_owned_windows_dialog(child: isize) {
    use windows_sys::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{
            GW_OWNER, GetWindow, GetWindowRect, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            SetWindowPos,
        },
    };
    let child = child as *mut std::ffi::c_void;
    let mut parent_rect = RECT::default();
    let mut child_rect = RECT::default();
    unsafe {
        let owner = GetWindow(child, GW_OWNER);
        if !owner.is_null()
            && GetWindowRect(owner, &mut parent_rect) != 0
            && GetWindowRect(child, &mut child_rect) != 0
        {
            let x = parent_rect.left
                + (parent_rect.right - parent_rect.left - child_rect.right + child_rect.left) / 2;
            let y = parent_rect.top
                + (parent_rect.bottom - parent_rect.top - child_rect.bottom + child_rect.top) / 2;
            SetWindowPos(
                child,
                std::ptr::null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
            );
        }
    }
}

#[cfg(target_os = "linux")]
mod wayland {
    use wayland_backend::sys::client::{Backend, ObjectId};
    use wayland_client::{
        Connection, Dispatch, Proxy, QueueHandle,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{wl_registry::WlRegistry, wl_surface::WlSurface},
    };
    use wayland_protocols::xdg::foreign::zv2::client::{
        zxdg_exported_v2::{self, ZxdgExportedV2},
        zxdg_exporter_v2::ZxdgExporterV2,
    };

    pub(super) struct Export {
        exported: ZxdgExportedV2,
        connection: Connection,
        pub identifier: String,
    }

    impl Export {
        #[allow(unsafe_code)]
        pub fn new(
            surface: raw_window_handle::WaylandWindowHandle,
            display: raw_window_handle::WaylandDisplayHandle,
        ) -> Option<Self> {
            // The parent retains its Arc<Window> throughout this export, keeping
            // both foreign libwayland objects alive. This queue owns only its
            // export objects and does not dispatch the main window's events.
            let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
            let connection = Connection::from_backend(backend);
            let id = unsafe {
                ObjectId::from_ptr(WlSurface::interface(), surface.surface.as_ptr().cast())
            }
            .ok()?;
            let surface = WlSurface::from_id(&connection, id).ok()?;
            let (globals, mut queue) = registry_queue_init::<State>(&connection).ok()?;
            let handle = queue.handle();
            let exporter: ZxdgExporterV2 = globals.bind(&handle, 1..=1, ()).ok()?;
            let exported = exporter.export_toplevel(&surface, &handle, ());
            let mut state = State::default();
            let result = queue.roundtrip(&mut state);
            exporter.destroy();
            if result.is_err() || state.identifier.is_empty() || state.identifier.len() > 512 {
                exported.destroy();
                let _ = connection.flush();
                return None;
            }
            Some(Self {
                exported,
                connection,
                identifier: state.identifier,
            })
        }
    }

    impl Drop for Export {
        fn drop(&mut self) {
            self.exported.destroy();
            let _ = self.connection.flush();
        }
    }

    #[derive(Default)]
    struct State {
        identifier: String,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    wayland_client::delegate_noop!(State: ignore ZxdgExporterV2);

    impl Dispatch<ZxdgExportedV2, ()> for State {
        fn event(
            state: &mut Self,
            _: &ZxdgExportedV2,
            event: <ZxdgExportedV2 as Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zxdg_exported_v2::Event::Handle { handle } = event {
                state.identifier = handle;
            }
        }
    }
}
