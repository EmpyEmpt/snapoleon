//! Multi-window winit + egui runtime for tray apps, drawn with OpenGL or wgpu.
//!
//! [`run`] owns the event loop and keeps it alive with zero windows. All windows
//! share one graphics context (see the `render` module for the two backends); each has its own surface,
//! `egui::Context`, `egui_winit::State` and renderer. A window
//! repaints only on input or when its context requests it, so an idle shell
//! uses no CPU.
//!
//! The app implements [`ShellApp`]; callbacks receive a [`Shell`] to open and
//! manage windows, and other threads talk to the app through a [`Proxy`].

mod platform;
mod proxy;
mod render;
mod runner;
mod shell;
mod spec;
mod window;

pub use proxy::Proxy;
pub use render::NAME as RENDERER;
pub use runner::run;
pub use shell::{Shell, ShellApp};
pub use spec::{ShellConfig, WindowKey, WindowSpec};
