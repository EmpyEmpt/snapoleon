//! Interactive demo of the shell: a normal window, a cover overlay on every
//! monitor and a small always-on-top tool window.
//!
//! `SNAP_SHELL_DEMO_AUTO=1` runs a scripted sequence on Windows (typing,
//! screenshots into `target/smoke/snap-shell/`, CPU idle measurement) and quits
//! by itself. Without it the demo quits on the Quit button or after 20 s.

use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use egui::{Align2, Color32, FontId, Frame, Key, RichText};
use snap_core::{Point, t, tf};
use snap_shell::{Proxy, Shell, ShellApp, ShellConfig, WindowKey, WindowSpec, run};

enum Ev {
    Passthrough(bool),
    OpenTool,
    OpenOverlays,
    CloseOverlays,
    Quit,
}

struct Demo {
    main: WindowKey,
    text: String,
    auto: bool,
    focus_text: bool,
    opened: Vec<(String, WindowKey)>,
}

impl Demo {
    fn open_overlays(&mut self, sh: &mut Shell<Ev>) {
        if !sh.windows_with_tag("overlay").is_empty() {
            return;
        }
        for (i, (rect, _)) in sh.monitors().into_iter().enumerate() {
            let key =
                sh.open(WindowSpec { title: t("shell.demo.overlay_title").into_owned(), tag: "overlay".into(), ..WindowSpec::cover(rect) });
            self.opened.push((format!("overlay{i}"), key));
        }
        self.open_tool(sh);
    }

    fn open_tool(&mut self, sh: &mut Shell<Ev>) {
        if !sh.windows_with_tag("tool").is_empty() {
            return;
        }
        let tool = sh.open(WindowSpec {
            title: t("shell.demo.tool_title").into_owned(),
            size: Some((240.0, 90.0)),
            position: Some(Point::new(60, 60)),
            always_on_top: true,
            tool_window: true,
            rounded: true,
            tag: "tool".into(),
            ..WindowSpec::default()
        });
        self.opened.push(("tool".into(), tool));
        let hidden = sh.open(WindowSpec {
            title: "excluded".into(),
            size: Some((240.0, 90.0)),
            position: Some(Point::new(60, 300)),
            always_on_top: true,
            tool_window: true,
            exclude_from_capture: true,
            tag: "tool".into(),
            ..WindowSpec::default()
        });
        self.opened.push(("excluded".into(), hidden));
    }

    fn close_overlays(sh: &mut Shell<Ev>) {
        for key in sh.windows_with_tag("overlay").into_iter().chain(sh.windows_with_tag("tool")) {
            sh.close(key);
        }
    }

    fn report(&self, sh: &Shell<Ev>) {
        for (label, key) in &self.opened {
            match (sh.first_frame_time(*key), sh.outer_rect(*key)) {
                (Some(d), rect) => println!("{label}: first frame {:.1} ms, outer rect {rect:?}", d.as_secs_f64() * 1000.0),
                (None, _) => println!("{label}: closed"),
            }
        }
        if let Some(d) = sh.first_frame_time(self.main) {
            println!("main: first frame {:.1} ms (includes waiting for the shared GPU)", d.as_secs_f64() * 1000.0);
        }
    }
}

impl ShellApp<Ev> for Demo {
    fn init(&mut self, sh: &mut Shell<Ev>) {
        let proxy = sh.proxy();
        if self.auto {
            let proxy = proxy.clone();
            std::thread::spawn(move || auto_sequence(proxy));
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(20));
            proxy.send(Ev::Quit);
        });
        self.main = sh.open(WindowSpec {
            title: t("shell.demo.window_title").into_owned(),
            size: Some((520.0, 360.0)),
            min_size: Some((360.0, 240.0)),
            tag: "main".into(),
            ..WindowSpec::default()
        });
        self.opened.push(("main".into(), self.main));
    }

    fn event(&mut self, sh: &mut Shell<Ev>, ev: Ev) {
        match ev {
            Ev::OpenTool => self.open_tool(sh),
            Ev::Passthrough(on) => {
                for key in sh.windows_with_tag("tool") {
                    sh.set_passthrough(key, on);
                }
            }
            Ev::OpenOverlays => self.open_overlays(sh),
            Ev::CloseOverlays => {
                self.report(sh);
                Self::close_overlays(sh);
            }
            Ev::Quit => {
                self.report(sh);
                sh.quit();
            }
        }
    }

    fn ui(&mut self, sh: &mut Shell<Ev>, win: WindowKey, ctx: &egui::Context) {
        let is_overlay = sh.windows_with_tag("overlay").contains(&win);
        let is_tool = sh.windows_with_tag("tool").contains(&win);
        ctx.root_ui(|ui| {
            if is_overlay {
                let rect = sh.outer_rect(win);
                egui::CentralPanel::default().frame(Frame::new().fill(Color32::from_rgb(14, 17, 24))).show(ui, |ui| {
                    let c = ui.max_rect().center();
                    ui.painter().text(c, Align2::CENTER_CENTER, t("shell.demo.esc_to_close"), FontId::proportional(40.0), Color32::WHITE);
                    let info = format!("{win}: {rect:?}  x{:.2}", ui.ctx().pixels_per_point());
                    ui.painter().text(c + egui::vec2(0.0, 48.0), Align2::CENTER_CENTER, info, FontId::monospace(14.0), Color32::GRAY);
                    ui.painter().rect_stroke(
                        ui.max_rect(),
                        0.0,
                        egui::Stroke::new(2.0, Color32::from_rgb(255, 80, 80)),
                        egui::StrokeKind::Inside,
                    );
                });
                if ui.ctx().input(|i| i.key_pressed(Key::Escape)) {
                    Self::close_overlays(sh);
                }
            } else if is_tool {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.label(RichText::new(t("shell.demo.tool_hint")).strong());
                    if ui.button(t("shell.demo.close_overlay")).clicked() {
                        Self::close_overlays(sh);
                    }
                });
            } else {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading(t("shell.demo.intro"));
                    ui.add_space(8.0);
                    ui.label(t("shell.demo.text_label"));
                    let edit = ui
                        .add(egui::TextEdit::singleline(&mut self.text).hint_text(t("shell.demo.text_hint")).desired_width(f32::INFINITY));
                    if std::mem::take(&mut self.focus_text) {
                        edit.request_focus();
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("shell.demo.open_overlay")).clicked() {
                            self.open_overlays(sh);
                        }
                        if ui.button(t("shell.demo.quit")).clicked() {
                            sh.proxy().send(Ev::Quit);
                        }
                    });
                    if let Some(d) = sh.first_frame_time(win) {
                        ui.label(tf("shell.demo.first_frame", &[("ms", &format!("{:.1}", d.as_secs_f64() * 1000.0))]));
                    }
                });
            }
        });
    }
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/smoke/snap-shell")
}

fn powershell(script: &str) -> String {
    let file = std::env::temp_dir().join(format!("snap-shell-demo-{}.ps1", std::process::id()));
    if let Err(e) = std::fs::write(&file, script) {
        return format!("cannot write script: {e}");
    }
    let out = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(&file).output();
    let _ = std::fs::remove_file(&file);
    match out {
        Ok(out) => format!("{}{}", String::from_utf8_lossy(&out.stdout).trim(), String::from_utf8_lossy(&out.stderr).trim()),
        Err(e) => format!("powershell failed: {e}"),
    }
}

fn screenshot(name: &str) {
    let path = out_dir().join(name);
    let script = format!(
        r##"Add-Type -AssemblyName System.Windows.Forms,System.Drawing
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class Dpi {{ [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c); }}'
[Dpi]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$bmp.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png)
"virtual screen: $($b.Left),$($b.Top) $($b.Width)x$($b.Height)""##,
        path.display()
    );
    println!("{name}: {}", powershell(&script));
}

fn send_keys(keys: &str) {
    powershell(&format!("Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('{keys}')"));
}

fn cpu_ms() -> f64 {
    powershell(&format!("(Get-Process -Id {}).TotalProcessorTime.TotalMilliseconds", std::process::id()))
        .replace(',', ".")
        .parse()
        .unwrap_or(f64::NAN)
}

/// Scripted run for non-interactive verification (Windows only).
fn auto_sequence(proxy: Proxy<Ev>) {
    std::fs::create_dir_all(out_dir()).ok();
    let wait = |s: f32| std::thread::sleep(Duration::from_secs_f32(s));
    wait(1.0);
    send_keys("hello");
    send_keys("^a^c{END}^v");
    wait(0.3);
    screenshot("01-main.png");
    proxy.send(Ev::OpenTool);
    wait(0.8);
    screenshot("01b-tool.png");
    proxy.send(Ev::Passthrough(true));
    wait(0.8);
    screenshot("01c-passthrough.png");
    proxy.send(Ev::Passthrough(false));
    proxy.send(Ev::OpenOverlays);
    wait(1.2);
    screenshot("02-overlay.png");
    send_keys("{ESC}");
    wait(0.5);
    screenshot("03-after-esc.png");
    proxy.send(Ev::OpenOverlays);
    wait(0.8);
    proxy.send(Ev::CloseOverlays);
    wait(0.5);
    let (t0, c0) = (Instant::now(), cpu_ms());
    wait(3.0);
    let (t1, c1) = (Instant::now(), cpu_ms());
    println!("idle: {:.1} ms CPU over {:.1} s wall", c1 - c0, (t1 - t0).as_secs_f64());
    proxy.send(Ev::Quit);
}

fn main() -> snap_core::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let auto = std::env::var_os("SNAP_SHELL_DEMO_AUTO").is_some();
    let mut cfg = ShellConfig::new(snap_core::APP_ID);
    cfg.theme_init = Box::new(|ctx| {
        let mut fonts = egui::FontDefinitions::empty();
        fonts
            .font_data
            .insert("inter".into(), egui::FontData::from_static(include_bytes!("../../../assets/fonts/Inter-Regular.ttf")).into());
        fonts
            .font_data
            .insert("mono".into(), egui::FontData::from_static(include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf")).into());
        fonts.families.insert(egui::FontFamily::Proportional, vec!["inter".into()]);
        fonts.families.insert(egui::FontFamily::Monospace, vec!["mono".into()]);
        ctx.set_fonts(fonts);
        ctx.set_visuals(egui::Visuals::dark());
    });
    run(cfg, Demo { main: 0, text: String::new(), auto, focus_text: auto, opened: Vec::new() })
}
