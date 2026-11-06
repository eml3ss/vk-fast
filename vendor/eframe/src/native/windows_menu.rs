//! Native popup-menu palette, scoped to this process. Resolve optional system
//! APIs by OS build; unsupported systems keep their standard menu rendering.
#![allow(unsafe_code)]

use std::sync::LazyLock;
use windows_sys::{
    Win32::{
        System::LibraryLoader::{
            GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
        },
        UI::{
            Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
            WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW},
        },
    },
    s, w,
};

type Preferred = unsafe extern "system" fn(i32) -> i32;
type Allow = unsafe extern "system" fn(bool) -> bool;
type Flush = unsafe extern "system" fn();
enum Setter {
    Preferred(Preferred),
    Allow(Allow),
}
struct Api {
    setter: Setter,
    flush: Option<Flush>,
}

fn load() -> Option<Api> {
    type Version = unsafe extern "system" fn(*mut u32, *mut u32, *mut u32);
    // The version query writes only these three live DWORDs. It avoids the
    // compatibility version returned for an unmanifested legacy application.
    let ntdll = unsafe { GetModuleHandleW(w!("ntdll.dll")) };
    if ntdll.is_null() {
        return None;
    }
    let version = unsafe { GetProcAddress(ntdll, s!("RtlGetNtVersionNumbers")) }?;
    let version: Version = unsafe { std::mem::transmute(version) };
    let (mut major, mut minor, mut build) = (0, 0, 0);
    unsafe {
        version(&mut major, &mut minor, &mut build);
    }
    build &= 0x0fff_ffff;
    if major != 10 || build < 17763 {
        return None;
    }
    // Load only the system copy. Retain the module for the lifetime of these
    // cached function pointers; no caller-supplied DLL path is searched.
    let module = unsafe {
        LoadLibraryExW(
            w!("uxtheme.dll"),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if module.is_null() {
        return None;
    }
    let setter = unsafe { GetProcAddress(module, 135usize as *const u8) }?;
    let setter = if build >= 18362 {
        // Ordinal 135 changed signature in Windows 10 1903.
        Setter::Preferred(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Preferred>(setter)
        })
    } else {
        Setter::Allow(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Allow>(setter)
        })
    };
    let flush = unsafe { GetProcAddress(module, 136usize as *const u8) }.map(|function| unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, Flush>(function)
    });
    Some(Api { setter, flush })
}

pub(crate) fn apply(dark: bool) -> bool {
    static API: LazyLock<Option<Api>> = LazyLock::new(load);
    let Some(api) = &*API else {
        return false;
    };
    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        dwFlags: 0,
        lpszDefaultScheme: std::ptr::null_mut(),
    };
    // This is a read-only accessibility query. Never override high-contrast
    // colors with a forced light/dark application palette.
    let high_contrast = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            size_of::<HIGHCONTRASTW>() as u32,
            (&raw mut contrast).cast(),
            0,
        )
    } != 0
        && contrast.dwFlags & HCF_HIGHCONTRASTON != 0;
    unsafe {
        match api.setter {
            Setter::Preferred(set) => {
                set(if high_contrast {
                    0
                } else if dark {
                    2
                } else {
                    3
                });
            }
            Setter::Allow(set) => {
                set(dark && !high_contrast);
            }
        }
        if let Some(flush) = api.flush {
            flush();
        }
    }
    true
}
