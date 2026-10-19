//! Linux (X11 and XWayland): the XRecord extension reports every core ButtonPress without grabbing anything.
//! Clicks on native Wayland windows are invisible to X, so step capture falls back to the manual shot there.

use std::{sync::Arc, thread::JoinHandle};

use snap_core::{Error, Point, Result, t};
use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::record::{self, ConnectionExt as _},
    rust_connection::RustConnection,
};

/// Core protocol event code of ButtonPress, and the button number of the primary button.
const BUTTON_PRESS: u8 = 4;
const PRIMARY_BUTTON: u8 = 1;
/// `EnableContextReply::category` values.
const FROM_SERVER: u8 = 0;
const END_OF_DATA: u8 = 5;
/// Size of a core event on the wire.
const EVENT_LEN: usize = 32;
/// Byte offsets of root_x / root_y inside a core input event.
const ROOT_X: usize = 20;
const ROOT_Y: usize = 22;

pub(crate) struct Hook {
    control: Arc<RustConnection>,
    context: record::Context,
    join: Option<JoinHandle<()>>,
}

impl Hook {
    pub(crate) fn start(on_click: Box<dyn Fn(Point) + Send>) -> Result<Hook> {
        let unsupported = || Error::unsupported(t("engine.error.no_click_listener").into_owned());
        // The record context is enabled on a dedicated connection: that connection is busy streaming events.
        let (control, _) = RustConnection::connect(None).map_err(|_| unsupported())?;
        let (data, _) = RustConnection::connect(None).map_err(|_| unsupported())?;
        if control.extension_information(record::X11_EXTENSION_NAME).ok().flatten().is_none() {
            return Err(unsupported());
        }
        let none = record::Range8 { first: 0, last: 0 };
        let no_ext = record::ExtRange { major: none, minor: record::Range16 { first: 0, last: 0 } };
        let range = record::Range {
            core_requests: none,
            core_replies: none,
            ext_requests: no_ext,
            ext_replies: no_ext,
            delivered_events: none,
            device_events: record::Range8 { first: BUTTON_PRESS, last: BUTTON_PRESS },
            errors: none,
            client_started: false,
            client_died: false,
        };
        let context = control.generate_id().map_err(|e| Error::Other(e.into()))?;
        control
            .record_create_context(context, 0, &[record::CS::ALL_CLIENTS.into()], &[range])
            .map_err(|e| Error::Other(e.into()))?
            .check()
            .map_err(|e| Error::Other(e.into()))?;
        let join = std::thread::Builder::new().name("snap-click-record".into()).spawn(move || {
            let Ok(stream) = data.record_enable_context(context) else { return };
            for reply in stream {
                let Ok(reply) = reply else { break };
                let d = &reply.data;
                match reply.category {
                    END_OF_DATA => break,
                    FROM_SERVER if d.len() >= EVENT_LEN && d[0] & 0x7f == BUTTON_PRESS && d[1] == PRIMARY_BUTTON => {
                        let coord = |at: usize| i32::from(i16::from_ne_bytes([d[at], d[at + 1]]));
                        on_click(Point::new(coord(ROOT_X), coord(ROOT_Y)));
                    }
                    _ => {}
                }
            }
        })?;
        Ok(Hook { control: Arc::new(control), context, join: Some(join) })
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // Disabling the context ends the stream; the thread sees EndOfData and returns.
        let _ = self.control.record_disable_context(self.context).map(|cookie| cookie.check());
        let _ = self.control.flush();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub(crate) fn is_own_window_at(_p: Point) -> bool {
    false
}
