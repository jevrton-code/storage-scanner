//! Ações em dispositivos MTP: excluir e copiar para o PC.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use windows::Win32::Devices::PortableDevices::{
    IPortableDevice, IPortableDevicePropVariantCollection, PortableDevicePropVariantCollection,
    WPD_CONTENT_TYPE_FOLDER, WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT, WPD_OBJECT_CONTENT_TYPE,
    WPD_OBJECT_ORIGINAL_FILE_NAME, WPD_RESOURCE_DEFAULT,
};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, STGM_READ};
use windows::core::PCWSTR;

use super::device::{Com, MtpDevice, lpwstr_variant, mtp_err, open, take_pwstr};
use crate::ScanError;

/// Exclui permanentemente o objeto (o MTP não tem Lixeira).
pub fn delete_object(dev: &MtpDevice, object_id: &str) -> Result<(), ScanError> {
    let (dev, object_id) = (dev.clone(), object_id.to_string());
    run_in_thread(move || {
        let device = open(&dev.id)?;
        delete_open(&device, &object_id)
    })
}

fn delete_open(device: &IPortableDevice, object_id: &str) -> Result<(), ScanError> {
    unsafe {
        let content = device.Content().map_err(mtp_err)?;
        let coll: IPortableDevicePropVariantCollection = CoCreateInstance(
            &PortableDevicePropVariantCollection,
            None,
            CLSCTX_INPROC_SERVER,
        )
        .map_err(mtp_err)?;
        let mut pv: PROPVARIANT = lpwstr_variant(object_id);
        let added = coll.Add(&pv);
        let _ = PropVariantClear(&mut pv);
        added.map_err(mtp_err)?;
        // PORTABLE_DEVICE_DELETE_NO_RECURSION = 0: pastas não vazias precisam ser esvaziadas antes;
        // com recursão (1) o aparelho apaga o conteúdo também.
        let mut results = None;
        content.Delete(1, &coll, &mut results).map_err(mtp_err)?;
    }
    Ok(())
}

/// Copia o objeto (arquivo ou pasta, recursivamente) para `dest_dir` no PC.
pub fn copy_object_to_pc(
    dev: &MtpDevice,
    object_id: &str,
    dest_dir: &Path,
) -> Result<(), ScanError> {
    let (dev, object_id, dest) = (dev.clone(), object_id.to_string(), dest_dir.to_path_buf());
    run_in_thread(move || {
        let device = open(&dev.id)?;
        copy_open(&device, &object_id, &dest)
    })
}

fn run_in_thread<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ScanError> + Send + 'static,
) -> Result<T, ScanError> {
    std::thread::spawn(move || {
        let _com = Com::init();
        f()
    })
    .join()
    .map_err(|_| ScanError::Mtp("falha interna na operação do dispositivo".into()))?
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if s.is_empty() { "_".into() } else { s }
}

fn copy_open(device: &IPortableDevice, object_id: &str, dest_dir: &Path) -> Result<(), ScanError> {
    unsafe {
        let content = device.Content().map_err(mtp_err)?;
        let props = content.Properties().map_err(mtp_err)?;
        let keys: windows::Win32::Devices::PortableDevices::IPortableDeviceKeyCollection =
            CoCreateInstance(
                &windows::Win32::Devices::PortableDevices::PortableDeviceKeyCollection,
                None,
                CLSCTX_INPROC_SERVER,
            )
            .map_err(mtp_err)?;
        keys.Add(&WPD_OBJECT_ORIGINAL_FILE_NAME).map_err(mtp_err)?;
        keys.Add(&WPD_OBJECT_CONTENT_TYPE).map_err(mtp_err)?;
        let w: Vec<u16> = object_id.encode_utf16().chain(Some(0)).collect();
        let values = props
            .GetValues(PCWSTR(w.as_ptr()), &keys)
            .map_err(mtp_err)?;
        let name = values
            .GetStringValue(&WPD_OBJECT_ORIGINAL_FILE_NAME)
            .map(|p| take_pwstr(p))
            .unwrap_or_else(|_| object_id.to_string());
        let ctype = values
            .GetGuidValue(&WPD_OBJECT_CONTENT_TYPE)
            .unwrap_or_default();
        let target = dest_dir.join(sanitize(&name));

        if ctype == WPD_CONTENT_TYPE_FOLDER || ctype == WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT {
            std::fs::create_dir_all(&target)?;
            let en = content
                .EnumObjects(0, PCWSTR(w.as_ptr()), None)
                .map_err(mtp_err)?;
            loop {
                let mut batch = [windows::core::PWSTR::null(); 32];
                let mut fetched = 0u32;
                let hr = en.Next(&mut batch, &mut fetched);
                let kids: Vec<String> = batch
                    .iter()
                    .take(fetched as usize)
                    .map(|p| take_pwstr(*p))
                    .collect();
                for k in kids {
                    copy_open(device, &k, &target)?;
                }
                if hr.is_err() || fetched == 0 || hr != windows::Win32::Foundation::S_OK {
                    break;
                }
            }
            return Ok(());
        }

        // Arquivo: lê o recurso padrão como fluxo e grava em disco.
        let resources = content.Transfer().map_err(mtp_err)?;
        let mut optimal = 0u32;
        let mut stream = None;
        resources
            .GetStream(
                PCWSTR(w.as_ptr()),
                &WPD_RESOURCE_DEFAULT,
                STGM_READ.0,
                &mut optimal,
                &mut stream,
            )
            .map_err(mtp_err)?;
        let stream = stream.ok_or_else(|| ScanError::Mtp("sem fluxo de dados".into()))?;
        let mut out = File::create(&target)?;
        let mut buf = vec![0u8; optimal.max(64 * 1024) as usize];
        loop {
            let mut read = 0u32;
            let hr = stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read));
            if hr.is_err() {
                let _ = std::fs::remove_file(&target);
                return Err(ScanError::Mtp(windows::core::Error::from(hr).message()));
            }
            if read == 0 {
                break;
            }
            out.write_all(&buf[..read as usize])?;
        }
    }
    Ok(())
}
