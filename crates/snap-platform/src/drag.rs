//! Dragging files out of a Snapoleon window into another program: a chat, a file dialog, the desktop.
//!
//! The OS runs the whole drag in a loop of its own, so [`files`] returns only when the user has dropped or given up.

use std::path::PathBuf;

use snap_core::Result;

/// Whether this OS can start a file drag from a window.
pub const SUPPORTED: bool = cfg!(windows);

/// Start dragging `paths` with the left mouse button held down, and wait for the drop. `true` when something took
/// the files, `false` when the user cancelled. Call it from the pointer handler that saw the drag begin; the
/// program that receives the drop gets copies, the originals stay where they are.
pub fn files(paths: &[PathBuf]) -> Result<bool> {
    imp::files(paths)
}

#[cfg(windows)]
mod imp {
    use std::{mem::ManuallyDrop, path::PathBuf};

    use snap_core::{Error, Result};
    use windows::{
        Win32::{
            Foundation::{
                DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC, DV_E_TYMED, E_NOTIMPL, HGLOBAL,
                OLE_E_ADVISENOTSUPPORTED, S_OK,
            },
            System::{
                Com::{
                    DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, STGMEDIUM,
                    STGMEDIUM_0, TYMED_HGLOBAL,
                },
                Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
                Ole::{CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize},
                SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS},
            },
            UI::Shell::SHCreateStdEnumFmtEtc,
        },
        core::{BOOL, HRESULT, Ref, implement},
    };

    pub fn files(paths: &[PathBuf]) -> Result<bool> {
        if paths.is_empty() {
            return Err(Error::invalid("no files to drag"));
        }
        let os = |e: windows::core::Error| Error::Other(anyhow::anyhow!("drag and drop: {e}"));
        let paths: Vec<PathBuf> = paths.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect();
        let data: IDataObject = Files { hdrop: hdrop(&paths) }.into();
        let source: IDropSource = Source.into();
        let mut effect = DROPEFFECT::default();
        // SAFETY: both objects outlive the call, and `effect` is a valid out pointer. OLE may already be initialised
        // on this thread (the window system does it), which is not an error.
        let result = unsafe {
            OleInitialize(None).map_err(os)?;
            DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect)
        };
        if result == DRAGDROP_S_DROP {
            Ok(true)
        } else if result == DRAGDROP_S_CANCEL {
            Ok(false)
        } else {
            Err(os(result.into()))
        }
    }

    /// The `CF_HDROP` payload: a `DROPFILES` header, then each wide path ended by a NUL, then one more NUL.
    fn hdrop(paths: &[PathBuf]) -> Vec<u8> {
        const HEADER: u32 = 20;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&HEADER.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]); // pt
        bytes.extend_from_slice(&0u32.to_le_bytes()); // fNC
        bytes.extend_from_slice(&1u32.to_le_bytes()); // fWide
        for path in paths {
            for unit in path.as_os_str().to_string_lossy().encode_utf16().chain([0]) {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&[0, 0]);
        bytes
    }

    /// A data object that offers the files as `CF_HDROP`, which is what Explorer, browsers and chat apps take.
    #[implement(IDataObject)]
    struct Files {
        hdrop: Vec<u8>,
    }

    fn format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    impl IDataObject_Impl for Files_Impl {
        fn GetData(&self, pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
            // SAFETY: OLE passes a valid FORMATETC.
            let asked = unsafe { &*pformatetcin };
            if asked.cfFormat != CF_HDROP.0 || asked.dwAspect & DVASPECT_CONTENT.0 == 0 {
                return Err(DV_E_FORMATETC.into());
            }
            if asked.tymed & TYMED_HGLOBAL.0 as u32 == 0 {
                return Err(DV_E_TYMED.into());
            }
            // SAFETY: the block is sized for the copy and unlocked before it is handed over; the receiver frees it.
            let global = unsafe {
                let global = GlobalAlloc(GMEM_MOVEABLE, self.hdrop.len())?;
                let target = GlobalLock(global).cast::<u8>();
                if target.is_null() {
                    return Err(windows::core::Error::from_thread());
                }
                std::ptr::copy_nonoverlapping(self.hdrop.as_ptr(), target, self.hdrop.len());
                let _ = GlobalUnlock(global);
                global
            };
            Ok(STGMEDIUM {
                tymed: TYMED_HGLOBAL.0 as u32,
                u: STGMEDIUM_0 { hGlobal: HGLOBAL(global.0) },
                pUnkForRelease: ManuallyDrop::new(None),
            })
        }

        fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
            // SAFETY: OLE passes a valid FORMATETC.
            let asked = unsafe { &*pformatetc };
            if asked.cfFormat != CF_HDROP.0 || asked.dwAspect & DVASPECT_CONTENT.0 == 0 {
                DV_E_FORMATETC
            } else if asked.tymed & TYMED_HGLOBAL.0 as u32 == 0 {
                DV_E_TYMED
            } else {
                S_OK
            }
        }

        fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> HRESULT {
            E_NOTIMPL
        }

        fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
            // DATADIR_GET
            if direction != 1 {
                return Err(E_NOTIMPL.into());
            }
            // SAFETY: the slice is copied by the call.
            unsafe { SHCreateStdEnumFmtEtc(&[format()]) }
        }

        fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<IAdviseSink>) -> windows::core::Result<u32> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }
    }

    /// Ends the drag when the left button comes up or Escape is pressed.
    #[implement(IDropSource)]
    struct Source;

    impl IDropSource_Impl for Source_Impl {
        fn QueryContinueDrag(&self, escape_pressed: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
            if escape_pressed.as_bool() {
                DRAGDROP_S_CANCEL
            } else if keys.0 & MK_LBUTTON.0 == 0 {
                DRAGDROP_S_DROP
            } else {
                S_OK
            }
        }

        fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;

    use snap_core::{Error, Result};

    pub fn files(_: &[PathBuf]) -> Result<bool> {
        Err(Error::unsupported("dragging files out of a window on this OS"))
    }
}
