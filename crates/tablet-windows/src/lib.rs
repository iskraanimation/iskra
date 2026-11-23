//! Pen tablets on Windows (MILESTONES.md, *The Windows pen backend*). What there is to read a
//! pen by, said in the log when the app starts: the pen or touch input the system reports,
//! which is what Windows Ink gives, and whether the tablet's driver offers WinTab, with each of
//! its devices and the range of its pressure. And the pen read through WinTab, which is how it
//! is read with Windows Ink off: a context opened on the app's window, whose packets are taken
//! each frame. WinTab's numbers are from its public specification.

#[cfg(windows)]
pub use windows::{Taken, WinTab};

/// A pen as WinTab reads it, in the units the app's pen samples have.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// 0 to 1; 0 while hovering.
    pub pressure: f32,
    /// In degrees, -90 to 90: to the right, and towards the user.
    pub tilt_x: f32,
    pub tilt_y: f32,
    /// In degrees, 0 to 360, clockwise.
    pub rotation: f32,
    pub eraser: bool,
}

/// What a WinTab packet says of the pen: its pressure within the device's `range`; its
/// orientation, in tenths of a degree, the way it leans (clockwise from the tablet's top), how
/// far it stands up from the tablet (900 upright), and its twist; and the cursor it is. A
/// driver numbers a tablet's cursors in threes, a puck, the pen's tip and its eraser.
pub fn reading(pressure: u32, range: (i32, i32), azimuth: i32, altitude: i32, twist: i32, cursor: u32) -> Reading {
    let (low, high) = range;
    let span = f64::from(high - low).max(1.0);
    let pressure = ((f64::from(pressure as i32 - low) / span).clamp(0.0, 1.0)) as f32;
    // Leaning by the angle from upright, towards where it points: across, and towards the
    // user, who is at the tablet's bottom.
    let lean = (90.0 - f64::from(altitude.unsigned_abs().min(900)) / 10.0).to_radians();
    let towards = (f64::from(azimuth) / 10.0).to_radians();
    let tilt_x = (lean.tan() * towards.sin()).atan().to_degrees() as f32;
    let tilt_y = (-(lean.tan() * towards.cos())).atan().to_degrees() as f32;
    let rotation = (f64::from(twist) / 10.0).rem_euclid(360.0) as f32;
    Reading { pressure, tilt_x, tilt_y, rotation, eraser: cursor % 3 == 2 }
}

/// What there is to read a pen by, a line each for the log.
pub fn report() -> Vec<String> {
    #[cfg(windows)]
    {
        windows::report()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// The pen and touch input the system reports, from the flags of `SM_DIGITIZER`.
pub fn digitizer(flags: i32) -> String {
    let kinds: Vec<&str> = [(0x04, "a pen built in"), (0x08, "a pen apart"), (0x01, "touch built in"), (0x02, "touch apart")]
        .into_iter()
        .filter(|(flag, _)| flags & flag != 0)
        .map(|(_, kind)| kind)
        .collect();
    match (kinds.is_empty(), flags & 0x80 != 0) {
        (true, _) => "Windows Ink: the system reports no pen and no touch input, so pressure can only come through WinTab".to_string(),
        (false, ready) => format!("Windows Ink: the system reports {}{}", kinds.join(", "), if ready { ", ready" } else { ", not ready" }),
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_DIGITIZER};

    use super::Reading;

    type Void = *mut core::ffi::c_void;
    /// `UINT WTInfoW(UINT wCategory, UINT nIndex, LPVOID lpOutput)`.
    type WtInfo = unsafe extern "system" fn(u32, u32, Void) -> u32;
    /// `HCTX WTOpenW(HWND hWnd, LPLOGCONTEXTW lpLogCtx, BOOL fEnable)`.
    type WtOpen = unsafe extern "system" fn(Void, *mut LogContext, i32) -> Void;
    /// `BOOL WTClose(HCTX hCtx)`.
    type WtClose = unsafe extern "system" fn(Void) -> i32;
    /// `int WTPacketsGet(HCTX hCtx, int cMaxPkts, LPVOID lpPkts)`.
    type WtPacketsGet = unsafe extern "system" fn(Void, i32, Void) -> i32;
    /// `BOOL WTQueueSizeSet(HCTX hCtx, int nPkts)`.
    type WtQueueSizeSet = unsafe extern "system" fn(Void, i32) -> i32;

    const WTI_DEFSYSCTX: u32 = 4;
    const CXO_MESSAGES: u32 = 0x0004;
    const PK_STATUS: u32 = 0x0002;
    const PK_CURSOR: u32 = 0x0020;
    const PK_BUTTONS: u32 = 0x0040;
    const PK_NORMAL_PRESSURE: u32 = 0x0400;
    const PK_ORIENTATION: u32 = 0x1000;
    /// The cursor is out of the context: the pen is away from the tablet.
    const TPS_PROXIMITY: u32 = 0x0001;

    /// A WinTab context, `LOGCONTEXTW`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LogContext {
        name: [u16; 40],
        options: u32,
        status: u32,
        locks: u32,
        msg_base: u32,
        device: u32,
        pkt_rate: u32,
        pkt_data: u32,
        pkt_mode: u32,
        move_mask: u32,
        btn_dn_mask: u32,
        btn_up_mask: u32,
        in_org: [i32; 3],
        in_ext: [i32; 3],
        out_org: [i32; 3],
        out_ext: [i32; 3],
        sens: [u32; 3],
        sys_mode: i32,
        sys_org: [i32; 2],
        sys_ext: [i32; 2],
        sys_sens: [u32; 2],
    }

    /// A packet of the fields asked for, in the order of their bits: status, cursor, buttons,
    /// pressure, orientation.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Packet {
        status: u32,
        cursor: u32,
        buttons: u32,
        pressure: u32,
        azimuth: i32,
        altitude: i32,
        twist: i32,
    }

    /// A WinTab context open on the app's window, its packets taken each frame.
    pub struct WinTab {
        library: HMODULE,
        context: Void,
        close: WtClose,
        packets: WtPacketsGet,
        range: (i32, i32),
        /// The device's name, as its driver gives it.
        pub device: String,
    }

    /// What the packets since the last frame say: nothing, the pen, or that it went away.
    pub enum Taken {
        Nothing,
        Pen(Reading),
        Away,
    }

    impl WinTab {
        /// Open a context on the window `hwnd`, from the system's own, so that the pen goes on
        /// moving the pointer.
        pub fn open(hwnd: isize) -> Result<WinTab, String> {
            let name: Vec<u16> = "wintab32.dll".encode_utf16().chain([0]).collect();
            // SAFETY: a null-terminated name; the library is freed on every way out but success,
            // and then when the context is dropped.
            let library = unsafe { LoadLibraryW(name.as_ptr()) };
            if library.is_null() {
                return Err("wintab32.dll is not there".to_string());
            }
            let opened = Self::open_in(library, hwnd);
            if opened.is_err() {
                // SAFETY: loaded above, and nothing of it is kept.
                unsafe { FreeLibrary(library) };
            }
            opened
        }

        fn open_in(library: HMODULE, hwnd: isize) -> Result<WinTab, String> {
            // SAFETY: each name is null-terminated, in a library that is loaded, and each is cast
            // to its signature in WinTab's specification.
            let (info, open, close, packets, queue) = unsafe {
                let find =
                    |name: &core::ffi::CStr| GetProcAddress(library, name.as_ptr().cast()).ok_or(format!("wintab32.dll has no {}", name.to_string_lossy()));
                (
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, WtInfo>(find(c"WTInfoW")?),
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, WtOpen>(find(c"WTOpenW")?),
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, WtClose>(find(c"WTClose")?),
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, WtPacketsGet>(find(c"WTPacketsGet")?),
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, WtQueueSizeSet>(find(c"WTQueueSizeSet")?),
                )
            };
            // SAFETY: a null output asks for the size of the system's context, which must be the
            // structure's for it to be read into it.
            let size = unsafe { info(WTI_DEFSYSCTX, 0, std::ptr::null_mut()) } as usize;
            if size != std::mem::size_of::<LogContext>() {
                return Err(format!("its context is {size} bytes, not the {} of WinTab's specification", std::mem::size_of::<LogContext>()));
            }
            // SAFETY: all zeros is a valid context of integers, and WinTab fills it whole.
            let mut context: LogContext = unsafe { std::mem::zeroed() };
            // SAFETY: `context` has the size WinTab said.
            unsafe { info(WTI_DEFSYSCTX, 0, (&raw mut context).cast()) };
            let title: Vec<u16> = "Iskra".encode_utf16().collect();
            context.name = [0; 40];
            context.name[..title.len()].copy_from_slice(&title);
            context.options &= !CXO_MESSAGES;
            context.pkt_data = PK_STATUS | PK_CURSOR | PK_BUTTONS | PK_NORMAL_PRESSURE | PK_ORIENTATION;
            context.pkt_mode = 0;
            let range = value::<Axis>(info, WTI_DEVICES + context.device, DVC_NPRESSURE).map_or((0, 1023), |axis| (axis.min, axis.max));
            let device = text(info, WTI_DEVICES + context.device, DVC_NAME).unwrap_or_else(|| "a tablet".to_string());
            // SAFETY: a window of this process, and a context filled by WinTab itself.
            let opened = unsafe { open(hwnd as Void, &raw mut context, 1) };
            if opened.is_null() {
                return Err("it would not open a context on the window".to_string());
            }
            // SAFETY: the context was just opened.
            unsafe { queue(opened, 128) };
            Ok(WinTab { library, context: opened, close, packets, range, device })
        }

        /// The packets since the last time, the newest of them.
        pub fn take(&mut self) -> Taken {
            let mut got = [Packet::default(); 64];
            let mut last = None;
            loop {
                // SAFETY: room for as many packets as asked, of the fields the context was
                // opened with.
                let count = unsafe { (self.packets)(self.context, got.len() as i32, got.as_mut_ptr().cast()) };
                if count <= 0 {
                    break;
                }
                last = Some(got[count as usize - 1]);
                if (count as usize) < got.len() {
                    break;
                }
            }
            match last {
                None => Taken::Nothing,
                Some(packet) if packet.status & TPS_PROXIMITY != 0 => Taken::Away,
                Some(packet) => Taken::Pen(super::reading(packet.pressure, self.range, packet.azimuth, packet.altitude, packet.twist, packet.cursor)),
            }
        }
    }

    impl Drop for WinTab {
        fn drop(&mut self) {
            // SAFETY: opened, and then loaded, by `open`, and used no more.
            unsafe {
                (self.close)(self.context);
                FreeLibrary(self.library);
            }
        }
    }

    // From WinTab's specification: the categories and indices asked about.
    const WTI_INTERFACE: u32 = 1;
    const IFC_WINTABID: u32 = 1;
    const IFC_NDEVICES: u32 = 4;
    const IFC_NCURSORS: u32 = 5;
    const WTI_DEVICES: u32 = 100;
    const DVC_NAME: u32 = 1;
    const DVC_NPRESSURE: u32 = 15;

    /// A WinTab axis: its least and most value, its units and resolution.
    #[repr(C)]
    #[derive(Default)]
    struct Axis {
        min: i32,
        max: i32,
        units: u32,
        resolution: u32,
    }

    pub fn report() -> Vec<String> {
        // SAFETY: GetSystemMetrics takes no pointers.
        let flags = unsafe { GetSystemMetrics(SM_DIGITIZER) };
        let mut said = vec![super::digitizer(flags)];
        said.extend(wintab());
        said
    }

    /// A text WinTab gives, in UTF-16: asked its size first, then read whole.
    fn text(info: WtInfo, category: u32, index: u32) -> Option<String> {
        // SAFETY: a null output asks only for the size, in bytes.
        let bytes = unsafe { info(category, index, std::ptr::null_mut()) } as usize;
        if bytes == 0 {
            return None;
        }
        let mut wide = vec![0u16; bytes.div_ceil(2) + 1];
        // SAFETY: the buffer holds the bytes WinTab said it needs.
        unsafe { info(category, index, wide.as_mut_ptr().cast()) };
        let end = wide.iter().position(|unit| *unit == 0).unwrap_or(wide.len());
        Some(String::from_utf16_lossy(&wide[..end]).trim().to_string())
    }

    /// A value of a fixed size WinTab gives, if it gives as many bytes.
    fn value<T: Default>(info: WtInfo, category: u32, index: u32) -> Option<T> {
        // SAFETY: a null output asks only for the size, in bytes.
        let bytes = unsafe { info(category, index, std::ptr::null_mut()) } as usize;
        if bytes == 0 || bytes > std::mem::size_of::<T>() {
            return None;
        }
        let mut out = T::default();
        // SAFETY: `out` has room for the bytes WinTab said it gives.
        unsafe { info(category, index, (&raw mut out).cast()) };
        Some(out)
    }

    fn wintab() -> Vec<String> {
        let name: Vec<u16> = "wintab32.dll".encode_utf16().chain([0]).collect();
        // SAFETY: a null-terminated name; the library is freed below.
        let library = unsafe { LoadLibraryW(name.as_ptr()) };
        if library.is_null() {
            return vec!["WinTab: wintab32.dll is not there, so no tablet driver offers WinTab: with Windows Ink off there is no pressure".to_string()];
        }
        // SAFETY: a null-terminated name in a library that is loaded.
        let found = unsafe { GetProcAddress(library, c"WTInfoW".as_ptr().cast()) };
        let mut said = Vec::new();
        match found {
            None => said.push("WinTab: wintab32.dll is there, without WTInfoW".to_string()),
            Some(found) => {
                // SAFETY: WTInfoW has this signature in WinTab's specification.
                let info: WtInfo = unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, WtInfo>(found) };
                // SAFETY: all nulls ask whether WinTab's services are there at all.
                if unsafe { info(0, 0, std::ptr::null_mut()) } == 0 {
                    said.push("WinTab: wintab32.dll is there, and says its services are not: the tablet's driver may not be running".to_string());
                } else {
                    let id = text(info, WTI_INTERFACE, IFC_WINTABID).unwrap_or_else(|| "no name".to_string());
                    let devices: u32 = value(info, WTI_INTERFACE, IFC_NDEVICES).unwrap_or(0);
                    let cursors: u32 = value(info, WTI_INTERFACE, IFC_NCURSORS).unwrap_or(0);
                    said.push(format!("WinTab: offered by \"{id}\", with {devices} devices and {cursors} cursors"));
                    for device in 0..devices.min(8) {
                        let name = text(info, WTI_DEVICES + device, DVC_NAME).unwrap_or_else(|| "no name".to_string());
                        let pressure = value::<Axis>(info, WTI_DEVICES + device, DVC_NPRESSURE)
                            .map_or("no pressure".to_string(), |axis| format!("pressure {} to {}", axis.min, axis.max));
                        said.push(format!("WinTab device {}: \"{name}\", {pressure}", device + 1));
                    }
                }
            }
        }
        // SAFETY: loaded above, and nothing of it is kept.
        unsafe { FreeLibrary(library) };
        said
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packet_is_read_as_the_pens_pressure_tilt_twist_and_end() {
        // Upright, half pressed, the tip.
        let upright = reading(512, (0, 1023), 0, 900, 0, 1);
        assert!((upright.pressure - 0.5).abs() < 0.01 && upright.tilt_x.abs() < 1e-4 && upright.tilt_y.abs() < 1e-4 && !upright.eraser);
        // Leaning 45° to the right, then towards the tablet's top, away from the user.
        let right = reading(1023, (0, 1023), 900, 450, 0, 1);
        assert!((right.tilt_x - 45.0).abs() < 1e-3 && right.tilt_y.abs() < 1e-3 && right.pressure == 1.0, "{right:?}");
        let away = reading(0, (0, 1023), 0, 450, 0, 1);
        assert!((away.tilt_y + 45.0).abs() < 1e-3 && away.pressure == 0.0, "{away:?}");
        // Twisted, and the eraser end, whose altitude a driver may give below zero.
        let eraser = reading(300, (100, 1100), 0, -900, 4500, 5);
        assert!(eraser.eraser && (eraser.rotation - 90.0).abs() < 1e-3 && (eraser.pressure - 0.2).abs() < 1e-6, "{eraser:?}");
    }

    #[test]
    fn the_systems_pen_and_touch_are_said_in_words() {
        assert!(digitizer(0).contains("no pen and no touch"));
        assert_eq!(digitizer(0x08 | 0x80), "Windows Ink: the system reports a pen apart, ready");
        assert_eq!(digitizer(0x04 | 0x01), "Windows Ink: the system reports a pen built in, touch built in, not ready");
        assert!(report().is_empty() || cfg!(windows));
    }
}
