//! System sizes an application needs to draw its own native images, read
//! through a safe wrapper.
#![allow(unsafe_code)]

use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};

/// The side of a small icon, as the notification area draws it, in physical
/// pixels at the system DPI.
pub(crate) fn small_icon_size() -> Option<u32> {
    // SAFETY: a getter without side effects or pointer arguments.
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
    u32::try_from(size).ok().filter(|size| (8..=256).contains(size))
}
