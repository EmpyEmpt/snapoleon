//! MCP server over stdio (JSON-RPC 2.0, newline-delimited). Tools:
//! list_monitors, list_windows, capture_screen, capture_window,
//! capture_region, ocr_image, decode_codes, search_history, get_capture.

use snap_core::Result;

use crate::Engine;

pub fn serve_stdio(engine: Engine) -> Result<()> {
    let _ = engine;
    Err(crate::todo())
}
