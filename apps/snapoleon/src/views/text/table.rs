//! Tables inside recognised text: detection (the OCR engine writes a detected table as tab-separated rows),
//! the formats to copy it in, and a rendered preview.

use egui::{CornerRadius, FontId, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, Vec2};
use snap_ui::{Palette, text::body_strong_style, widgets::util};

use crate::views::library::paint;

const ROW_H: f32 = 28.0;
const CELL_PAD: f32 = 10.0;
const MIN_COL: f32 = 56.0;
const MAX_COL: f32 = 240.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Every row has `cols` cells; short rows are padded with empty ones.
    pub rows: Vec<Vec<String>>,
    pub cols: usize,
}

impl Table {
    /// The longest run of lines made of two or more tab-separated cells, if it has at least two rows.
    pub fn detect(text: &str) -> Option<Table> {
        let mut best: Vec<Vec<String>> = Vec::new();
        let mut run: Vec<Vec<String>> = Vec::new();
        for line in text.lines().chain(std::iter::once("")) {
            let cells: Vec<String> = line.split('\t').map(|c| c.trim().to_owned()).collect();
            if cells.len() >= 2 && cells.iter().any(|c| !c.is_empty()) {
                run.push(cells);
            } else {
                if run.len() > best.len() {
                    best = std::mem::take(&mut run);
                }
                run.clear();
            }
        }
        if best.len() < 2 {
            return None;
        }
        let cols = best.iter().map(Vec::len).max().unwrap_or(0);
        for row in &mut best {
            row.resize(cols, String::new());
        }
        Some(Table { rows: best, cols })
    }

    /// Tab-separated rows, which spreadsheets paste as cells.
    pub fn tsv(&self) -> String {
        self.rows.iter().map(|r| r.join("\t")).collect::<Vec<_>>().join("\n")
    }

    /// A GitHub-flavoured Markdown table with the first row as the header.
    pub fn markdown(&self) -> String {
        let line = |cells: &[String]| format!("| {} |", cells.iter().map(|c| c.replace('|', "\\|")).collect::<Vec<_>>().join(" | "));
        let mut out = vec![line(&self.rows[0]), format!("| {} |", vec!["---"; self.cols].join(" | "))];
        out.extend(self.rows[1..].iter().map(|r| line(r)));
        out.join("\n")
    }

    /// An HTML table, which Excel, Sheets and Word keep as a table when pasted.
    pub fn html(&self) -> String {
        let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let rows: String = self
            .rows
            .iter()
            .map(|r| format!("<tr>{}</tr>", r.iter().map(|c| format!("<td>{}</td>", escape(c))).collect::<String>()))
            .collect();
        format!("<table>{rows}</table>")
    }
}

/// Paint the table in a scroll area at most `max_height` tall.
pub fn show(ui: &mut Ui, table: &Table, max_height: f32) {
    let p = Palette::of(ui);
    let body = ui.style().text_styles[&snap_ui::text::body_style()].clone();
    let strong = body_strong_style().resolve(ui.style());

    let widths: Vec<f32> = (0..table.cols)
        .map(|c| {
            let widest = table
                .rows
                .iter()
                .enumerate()
                .map(|(r, row)| ui.painter().layout_no_wrap(row[c].clone(), font_for(r, &body, &strong), p.text).size().x)
                .fold(0.0_f32, f32::max);
            (widest + CELL_PAD * 2.0).clamp(MIN_COL, MAX_COL)
        })
        .collect();
    let size = Vec2::new(widths.iter().sum(), table.rows.len() as f32 * ROW_H);

    egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).corner_radius(10).show(ui, |ui| {
        ScrollArea::both().max_height(max_height).auto_shrink([true, true]).show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(size.max(Vec2::new(ui.available_width(), 0.0)), Sense::hover());
            let painter = ui.painter();
            let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), ROW_H));
            painter.rect_filled(header, CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 }, p.surface_2);
            let mut y = rect.top();
            for (r, row) in table.rows.iter().enumerate() {
                let mut x = rect.left();
                for (c, cell) in row.iter().enumerate() {
                    let galley = paint::elided(ui, cell, font_for(r, &body, &strong), p.text, widths[c] - CELL_PAD * 2.0);
                    let at = Pos2::new(x + CELL_PAD, y + (ROW_H - galley.size().y) / 2.0);
                    ui.painter().galley(util::snap(ui, at), galley, p.text);
                    x += widths[c];
                    if c + 1 < table.cols {
                        ui.painter().vline(x - 0.5, y..=y + ROW_H, Stroke::new(1.0, p.border));
                    }
                }
                y += ROW_H;
                if r + 1 < table.rows.len() {
                    ui.painter().hline(rect.x_range(), y - 0.5, Stroke::new(1.0, p.border));
                }
            }
        });
    });
}

fn font_for(row: usize, body: &FontId, strong: &FontId) -> FontId {
    if row == 0 { strong.clone() } else { body.clone() }
}
