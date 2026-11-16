//! Enumeração recursiva dos objetos de um dispositivo WPD.
//!
//! Para cada pasta lista os ids dos filhos e lê as propriedades de todos de uma vez com
//! `IPortableDevicePropertiesBulk` (uma viagem ao aparelho em vez de uma por arquivo). Se o
//! dispositivo não suportar o modo em lote, ou ele falhar, cai para `GetValues` por objeto.
//! Erros em um objeto não abortam a varredura.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use ss_core::{NONE, NodeInfo, Tree, TreeBuilder, flags};
use windows::Win32::Devices::PortableDevices::{
    IEnumPortableDeviceObjectIDs, IPortableDevice, IPortableDeviceContent,
    IPortableDeviceKeyCollection, IPortableDevicePropVariantCollection, IPortableDeviceProperties,
    IPortableDevicePropertiesBulk, IPortableDevicePropertiesBulkCallback,
    IPortableDevicePropertiesBulkCallback_Impl, IPortableDeviceValues,
    IPortableDeviceValuesCollection, PortableDeviceKeyCollection,
    PortableDevicePropVariantCollection, WPD_CONTENT_TYPE_FOLDER,
    WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT, WPD_DEVICE_OBJECT_ID, WPD_OBJECT_CONTENT_TYPE,
    WPD_OBJECT_DATE_MODIFIED, WPD_OBJECT_ID, WPD_OBJECT_NAME, WPD_OBJECT_ORIGINAL_FILE_NAME,
    WPD_OBJECT_SIZE,
};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Variant::VT_DATE;
use windows::core::{GUID, HRESULT, PCWSTR, implement};

use super::device::{lpwstr_variant, mtp_err, take_pwstr};
use crate::{Progress, ScanError};

/// Ids dos objetos por id de nó (mesma numeração da `Tree`), para ações posteriores.
pub struct Scanned {
    pub tree: Tree,
    pub object_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Props {
    pub object_id: String,
    pub name: String,
    pub size: u64,
    /// FILETIME (0 se desconhecido).
    pub mtime: i64,
    pub is_dir: bool,
}

/// `VT_DATE` (dias desde 1899-12-30, fração = hora) → FILETIME.
pub fn vt_date_to_filetime(date: f64) -> i64 {
    // De 1601-01-01 a 1899-12-30 há 109 205 dias; um dia = 864 000 000 000 intervalos de 100 ns.
    if !date.is_finite() || date <= 0.0 {
        return 0;
    }
    ((date + 109_205.0) * 864_000_000_000.0) as i64
}

fn keys() -> Result<IPortableDeviceKeyCollection, ScanError> {
    unsafe {
        let k: IPortableDeviceKeyCollection =
            CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER)
                .map_err(mtp_err)?;
        for key in [
            WPD_OBJECT_ID,
            WPD_OBJECT_ORIGINAL_FILE_NAME,
            WPD_OBJECT_NAME,
            WPD_OBJECT_SIZE,
            WPD_OBJECT_DATE_MODIFIED,
            WPD_OBJECT_CONTENT_TYPE,
        ] {
            k.Add(&key).map_err(mtp_err)?;
        }
        Ok(k)
    }
}

/// Extrai `Props` de um conjunto de valores (de `GetValues` ou do callback em lote).
fn props_from_values(v: &IPortableDeviceValues, fallback_id: &str) -> Props {
    unsafe {
        let get_str = |k: &PROPERTYKEY| v.GetStringValue(k).ok().map(|p| take_pwstr(p));
        let object_id = get_str(&WPD_OBJECT_ID).unwrap_or_else(|| fallback_id.to_string());
        let name = get_str(&WPD_OBJECT_ORIGINAL_FILE_NAME)
            .filter(|s| !s.is_empty())
            .or_else(|| get_str(&WPD_OBJECT_NAME))
            .unwrap_or_default();
        let size = v
            .GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE)
            .unwrap_or(0);
        let ctype = v
            .GetGuidValue(&WPD_OBJECT_CONTENT_TYPE)
            .unwrap_or(GUID::zeroed());
        let is_dir =
            ctype == WPD_CONTENT_TYPE_FOLDER || ctype == WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT;
        let mtime = match v.GetValue(&WPD_OBJECT_DATE_MODIFIED) {
            Ok(mut pv) => {
                let inner = &pv.Anonymous.Anonymous;
                let t = if inner.vt == VT_DATE {
                    vt_date_to_filetime(inner.Anonymous.date)
                } else {
                    0
                };
                let _ = PropVariantClear(&mut pv);
                t
            }
            Err(_) => 0,
        };
        Props {
            object_id,
            name,
            size,
            mtime,
            is_dir,
        }
    }
}

// --------------------------------------------------------------- leitura em lote

enum BulkMsg {
    Values(Vec<IPortableDeviceValues>),
    End(HRESULT),
}

#[implement(IPortableDevicePropertiesBulkCallback)]
struct BulkCallback {
    tx: Sender<BulkMsg>,
}

impl IPortableDevicePropertiesBulkCallback_Impl for BulkCallback_Impl {
    fn OnStart(&self, _ctx: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnProgress(
        &self,
        _ctx: *const GUID,
        results: windows::core::Ref<'_, IPortableDeviceValuesCollection>,
    ) -> windows::core::Result<()> {
        if let Some(coll) = results.as_ref() {
            let mut out = Vec::new();
            unsafe {
                // O parâmetro é declarado `*const u32`, mas o COM grava nele: usa `Cell`.
                let n = std::cell::Cell::new(0u32);
                coll.GetCount(n.as_ptr())?;
                for i in 0..n.get() {
                    if let Ok(v) = coll.GetAt(i) {
                        out.push(v);
                    }
                }
            }
            let _ = self.tx.send(BulkMsg::Values(out));
        }
        Ok(())
    }

    fn OnEnd(&self, _ctx: *const GUID, status: HRESULT) -> windows::core::Result<()> {
        let _ = self.tx.send(BulkMsg::End(status));
        Ok(())
    }
}

struct Reader {
    props: IPortableDeviceProperties,
    bulk: Option<IPortableDevicePropertiesBulk>,
    keys: IPortableDeviceKeyCollection,
}

impl Reader {
    fn new(content: &IPortableDeviceContent) -> Result<Self, ScanError> {
        let props = unsafe { content.Properties() }.map_err(mtp_err)?;
        let bulk = windows::core::Interface::cast::<IPortableDevicePropertiesBulk>(&props).ok();
        Ok(Self {
            props,
            bulk,
            keys: keys()?,
        })
    }

    /// Lê as propriedades de `ids`: em lote quando possível, senão objeto a objeto.
    fn read(&mut self, ids: &[String], cancel: &AtomicBool) -> Vec<Props> {
        if ids.is_empty() {
            return Vec::new();
        }
        if let Some(bulk) = self.bulk.clone() {
            match self.read_bulk(&bulk, ids, cancel) {
                Some(v) if v.len() == ids.len() => return v,
                _ => {
                    tracing::debug!(
                        "leitura em lote indisponível/incompleta; usando objeto a objeto"
                    );
                    self.bulk = None;
                }
            }
        }
        ids.iter()
            .filter(|_| !cancel.load(Ordering::Relaxed))
            .map(|id| self.read_one(id))
            .collect()
    }

    fn read_one(&self, id: &str) -> Props {
        let w: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        match unsafe { self.props.GetValues(PCWSTR(w.as_ptr()), &self.keys) } {
            Ok(v) => props_from_values(&v, id),
            Err(e) => {
                tracing::debug!("propriedades de {id} indisponíveis: {e}");
                Props {
                    object_id: id.to_string(),
                    name: id.to_string(),
                    ..Props::default()
                }
            }
        }
    }

    fn read_bulk(
        &self,
        bulk: &IPortableDevicePropertiesBulk,
        ids: &[String],
        cancel: &AtomicBool,
    ) -> Option<Vec<Props>> {
        unsafe {
            let coll: IPortableDevicePropVariantCollection = CoCreateInstance(
                &PortableDevicePropVariantCollection,
                None,
                CLSCTX_INPROC_SERVER,
            )
            .ok()?;
            for id in ids {
                let mut pv: PROPVARIANT = lpwstr_variant(id);
                let r = coll.Add(&pv);
                let _ = PropVariantClear(&mut pv);
                r.ok()?;
            }
            let (tx, rx): (Sender<BulkMsg>, Receiver<BulkMsg>) = crossbeam_channel::unbounded();
            let cb: IPortableDevicePropertiesBulkCallback = BulkCallback { tx }.into();
            let ctx = bulk
                .QueueGetValuesByObjectList(&coll, &self.keys, &cb)
                .ok()?;
            bulk.Start(&ctx).ok()?;

            let mut got: Vec<Props> = Vec::with_capacity(ids.len());
            loop {
                if cancel.load(Ordering::Relaxed) {
                    let _ = bulk.Cancel(&ctx);
                    return None;
                }
                match rx.recv_timeout(Duration::from_secs(60)) {
                    Ok(BulkMsg::Values(vs)) => {
                        got.extend(vs.iter().map(|v| props_from_values(v, "")));
                    }
                    Ok(BulkMsg::End(hr)) => return hr.is_ok().then_some(got),
                    Err(_) => {
                        let _ = bulk.Cancel(&ctx);
                        return None;
                    }
                }
            }
        }
    }
}

/// Lista os ids dos filhos de `parent` (vazio se o objeto não puder ser enumerado).
fn child_ids(content: &IPortableDeviceContent, parent: &str) -> Vec<String> {
    let w: Vec<u16> = parent.encode_utf16().chain(Some(0)).collect();
    let Ok(en): Result<IEnumPortableDeviceObjectIDs, _> =
        (unsafe { content.EnumObjects(0, PCWSTR(w.as_ptr()), None) })
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    loop {
        let mut batch = [windows::core::PWSTR::null(); 64];
        let mut fetched = 0u32;
        let hr = unsafe { en.Next(&mut batch, &mut fetched) };
        for p in batch.iter().take(fetched as usize) {
            out.push(unsafe { take_pwstr(*p) });
        }
        // S_OK com lote cheio: há mais; S_FALSE ou 0 buscados: acabou.
        if hr.is_err() || fetched == 0 || hr != windows::Win32::Foundation::S_OK {
            break;
        }
    }
    out
}

/// Varre o dispositivo aberto. Chame numa thread com COM inicializado.
pub fn scan_open_device(
    device: &IPortableDevice,
    device_name: &str,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<Scanned, ScanError> {
    use std::sync::atomic::Ordering::Relaxed;
    let content = unsafe { device.Content() }.map_err(mtp_err)?;
    let mut reader = Reader::new(&content)?;

    let mut b = TreeBuilder::new();
    let dir = NodeInfo {
        flags: flags::DIR,
        ..Default::default()
    };
    let root = b.push(NONE, device_name, dir);
    let mut object_ids =
        vec![unsafe { WPD_DEVICE_OBJECT_ID.to_string() }.unwrap_or_else(|_| "DEVICE".into())];
    debug_assert_eq!(root, 0);

    // Fila de pastas a visitar: (id do nó, id do objeto).
    let mut queue: Vec<(u32, String)> = vec![(root, object_ids[0].clone())];
    while let Some((node, object)) = queue.pop() {
        if cancel.load(Relaxed) {
            return Err(ScanError::Cancelled);
        }
        let ids = child_ids(&content, &object);
        let props = reader.read(&ids, cancel);
        progress.dirs.fetch_add(1, Relaxed);
        for p in props {
            let name = if p.name.is_empty() {
                p.object_id.clone()
            } else {
                p.name.clone()
            };
            let info = NodeInfo {
                flags: if p.is_dir { flags::DIR } else { 0 },
                size: if p.is_dir { 0 } else { p.size },
                // MTP não expõe blocos: alocado = tamanho.
                allocated: if p.is_dir { 0 } else { p.size },
                mtime: p.mtime,
            };
            let id = b.push(node, &name, info);
            object_ids.push(p.object_id.clone());
            if p.is_dir {
                queue.push((id, p.object_id));
            } else {
                progress.files.fetch_add(1, Relaxed);
                progress.bytes.fetch_add(p.size, Relaxed);
            }
        }
    }
    Ok(Scanned {
        tree: b.finish(),
        object_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vt_date_conversion() {
        // 2024-01-02 03:04:00 UTC = 45293 dias + 3h04m desde 1899-12-30.
        let date = 45_293.0 + (3.0 * 3600.0 + 4.0 * 60.0) / 86_400.0;
        let ft = vt_date_to_filetime(date);
        assert_eq!(ss_core::format::format_filetime(ft), "02/01/2024 03:04");
        assert_eq!(vt_date_to_filetime(0.0), 0);
        assert_eq!(vt_date_to_filetime(f64::NAN), 0);
    }
}
