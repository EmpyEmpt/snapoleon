use snap_core::{Error, Point, Result, t};

pub(crate) struct Hook;

impl Hook {
    pub(crate) fn start(_on_click: Box<dyn Fn(Point) + Send>) -> Result<Hook> {
        Err(Error::unsupported(t("engine.error.no_click_listener").into_owned()))
    }
}

pub(crate) fn is_own_window_at(_p: Point) -> bool {
    false
}
