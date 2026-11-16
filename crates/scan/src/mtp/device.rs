//! Dispositivos WPD (celulares/câmeras via MTP): listagem e abertura.

use windows::Win32::Devices::PortableDevices::{
    IPortableDevice, IPortableDeviceManager, IPortableDeviceValues, PortableDeviceFTM,
    PortableDeviceManager, PortableDeviceValues, WPD_CLIENT_MAJOR_VERSION,
    WPD_CLIENT_MINOR_VERSION, WPD_CLIENT_NAME, WPD_CLIENT_REVISION,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::core::{PCWSTR, PWSTR};

use crate::ScanError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MtpDevice {
    /// Identificador PnP (usado para abrir o dispositivo).
    pub id: String,
    pub name: String,
}

pub(super) fn mtp_err(e: windows::core::Error) -> ScanError {
    ScanError::Mtp(e.message())
}

/// COM em modo multithread pela duração do guard (WPD funciona em qualquer apartment).
pub(super) struct Com(bool);

impl Com {
    pub fn init() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// Converte uma `PWSTR` alocada pelo COM em `String` e a libera.
pub(super) unsafe fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

/// Lista os dispositivos portáteis conectados (roda numa thread própria com COM inicializado).
pub fn list_devices() -> Result<Vec<MtpDevice>, ScanError> {
    std::thread::spawn(list_devices_inner)
        .join()
        .map_err(|_| ScanError::Mtp("falha interna ao listar dispositivos".into()))?
}

fn list_devices_inner() -> Result<Vec<MtpDevice>, ScanError> {
    let _com = Com::init();
    unsafe {
        let mgr: IPortableDeviceManager =
            CoCreateInstance(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER)
                .map_err(mtp_err)?;
        let mut count = 0u32;
        mgr.GetDevices(std::ptr::null_mut(), &mut count)
            .map_err(mtp_err)?;
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut ids = vec![PWSTR::null(); count as usize];
        mgr.GetDevices(ids.as_mut_ptr(), &mut count)
            .map_err(mtp_err)?;
        let mut out = Vec::new();
        for id in ids.into_iter().take(count as usize) {
            let id_s = take_pwstr(id);
            let wid: Vec<u16> = id_s.encode_utf16().chain(Some(0)).collect();
            let mut len = 0u32;
            let _ = mgr.GetDeviceFriendlyName(PCWSTR(wid.as_ptr()), PWSTR::null(), &mut len);
            let name = if len > 0 {
                let mut buf = vec![0u16; len as usize];
                match mgr.GetDeviceFriendlyName(
                    PCWSTR(wid.as_ptr()),
                    PWSTR(buf.as_mut_ptr()),
                    &mut len,
                ) {
                    Ok(()) => String::from_utf16_lossy(&buf[..buf.len().saturating_sub(1)]),
                    Err(_) => id_s.clone(),
                }
            } else {
                id_s.clone()
            };
            out.push(MtpDevice { id: id_s, name });
        }
        Ok(out)
    }
}

/// Abre o dispositivo (COM já deve estar inicializado na thread atual).
pub(super) fn open(device_id: &str) -> Result<IPortableDevice, ScanError> {
    unsafe {
        let dev: IPortableDevice =
            CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER).map_err(mtp_err)?;
        let info: IPortableDeviceValues =
            CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER).map_err(mtp_err)?;
        let name: Vec<u16> = "Storage Scanner".encode_utf16().chain(Some(0)).collect();
        info.SetStringValue(&WPD_CLIENT_NAME, PCWSTR(name.as_ptr()))
            .map_err(mtp_err)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1)
            .map_err(mtp_err)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_MINOR_VERSION, 0)
            .map_err(mtp_err)?;
        info.SetUnsignedIntegerValue(&WPD_CLIENT_REVISION, 0)
            .map_err(mtp_err)?;
        let wid: Vec<u16> = device_id.encode_utf16().chain(Some(0)).collect();
        dev.Open(PCWSTR(wid.as_ptr()), &info).map_err(mtp_err)?;
        Ok(dev)
    }
}

/// `PROPVARIANT` do tipo `VT_LPWSTR` com uma cópia (alocada no COM) de `s`.
/// Libere com `PropVariantClear`.
pub(super) fn lpwstr_variant(
    s: &str,
) -> windows::Win32::System::Com::StructuredStorage::PROPVARIANT {
    use std::mem::ManuallyDrop;
    use windows::Win32::System::Com::CoTaskMemAlloc;
    use windows::Win32::System::Com::StructuredStorage::{
        PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;

    let wide: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
    let bytes = wide.len() * 2;
    let ptr = unsafe { CoTaskMemAlloc(bytes) } as *mut u16;
    if !ptr.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len()) };
    }
    PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    pwszVal: PWSTR(ptr),
                },
            }),
        },
    }
}
