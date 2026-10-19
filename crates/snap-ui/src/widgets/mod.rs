//! Widgets. Free functions take `&mut Ui` and return an `egui::Response`; builders implement `egui::Widget`.

mod button;
mod chips;
mod color;
mod hotkey;
mod input;
mod layout;
mod stroke;
mod title_bar;
mod toggle;
mod toolbar;
mod tooltip;
pub mod util;

pub use button::{Button, ButtonKind, danger_button, ghost_button, icon_button, primary_button, secondary_button, tool_button};
pub use chips::{accel_parts, badge, chip, chip_with_icon, kbd};
pub use color::{ColorSwatches, DEFAULT_PALETTE, color_picker, color_swatches, hex_string, parse_color};
pub use hotkey::{RecorderState, hotkey_recorder};
pub use input::{Slider, TextField, dropdown, number_field, number_field_with, search_field, slider, text_field};
pub use layout::{
    card, card_frame, divider, empty_state, empty_state_brand, empty_state_with, nav_item, progress, progress_indeterminate,
    progress_sized, section, setting_row, spinner, spinner_sized,
};
pub use stroke::{DEFAULT_WIDTHS, stroke_picker, stroke_picker_with};
pub use title_bar::{TITLE_BAR_HEIGHT, TitleBarActions, title_bar, title_bar_with};
pub use toggle::{checkbox, segmented, toggle};
pub use toolbar::{floating_toolbar, floating_toolbar_anchored, toolbar_pill, toolbar_separator};
pub use tooltip::tooltip;
