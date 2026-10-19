//! The 3-2-1 tile shown over the recorded area before the recording starts.

use std::rc::Rc;

use egui::Context;

use super::{Shared, View, ViewCx};

pub struct TileView {
    pub shared: Rc<Shared>,
}

impl View for TileView {
    fn ui(&mut self, _cx: &mut ViewCx, ctx: &Context) {
        if let Some((number, into_second)) = self.shared.count.get() {
            crate::views::countdown::paint(ctx, number, into_second);
        }
    }
}
