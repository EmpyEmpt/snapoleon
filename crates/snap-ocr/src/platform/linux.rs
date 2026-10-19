//! Linux has no OS-level OCR service; `tesseract` and `ocrs` cover it.

use snap_core::{Error, Result, RgbaImage, t};

use crate::OcrResult;

pub(crate) fn recognize(_: &RgbaImage, _: &[String]) -> Result<OcrResult> {
    Err(Error::unsupported(t("ocr.note.native_unsupported").into_owned()))
}

pub(crate) fn languages() -> Vec<String> {
    Vec::new()
}

pub(crate) fn status() -> (bool, String) {
    (false, t("ocr.note.native_unsupported").into_owned())
}
