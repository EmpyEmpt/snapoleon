//! Reading-order text, paragraph breaks and table detection over OCR boxes.

use snap_core::RectF;

use crate::{OcrLine, OcrResult};

/// A vertical gap larger than this many line heights starts a new paragraph.
const PARAGRAPH_GAP: f32 = 0.85;
/// First-line indent (in line heights) that starts a paragraph after a sentence end.
const INDENT: f32 = 1.5;
/// A horizontal gap wider than this many line heights between lines sharing a row becomes a tab.
const TAB_GAP: f32 = 2.5;
/// A column separator must be at least this many word heights wide.
const COLUMN_GAP: f32 = 0.9;
/// Share of rows allowed to cross a column separator (titles, spanning cells).
const CROSSING_ROWS: f32 = 0.12;
/// Cells longer than this many words on average mean columns of prose, not a table.
const MAX_CELL_WORDS: f32 = 5.0;

impl OcrResult {
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.text.trim().is_empty())
    }

    /// Plain text with a blank line between paragraphs. Paragraphs are split
    /// on vertical gaps and on indented first lines; lines that sit on the
    /// same row (side-by-side blocks) are joined with a space or a tab, and a
    /// detected table is written row by row with tab-separated cells.
    pub fn text(&self) -> String {
        let table = detect_table(self);
        let in_table = |l: &OcrLine| table.as_ref().is_some_and(|t| t.bounds.contains(l.rect.center()));

        // (box, text) in reading order; the table collapses into one chunk.
        let mut chunks: Vec<(RectF, String)> = Vec::new();
        let mut table_done = false;
        for line in self.lines.iter().filter(|l| !l.text.trim().is_empty()) {
            if in_table(line) {
                if let (false, Some(t)) = (table_done, &table) {
                    table_done = true;
                    chunks.push((t.bounds, t.rows.iter().map(|r| r.join("\t")).collect::<Vec<_>>().join("\n")));
                }
            } else {
                chunks.push((line.rect, line.text.trim().to_string()));
            }
        }
        let Some(first) = chunks.first() else { return String::new() };
        let h = median(self.lines.iter().filter(|l| !in_table(l)).map(|l| l.rect.h)).max(1.0);

        let mut out = first.1.clone();
        for pair in chunks.windows(2) {
            let ((prev, prev_text), (cur, cur_text)) = (&pair[0], &pair[1]);
            let overlap = prev.bottom().min(cur.bottom()) - prev.y.max(cur.y);
            let same_row = overlap > 0.5 * prev.h.min(cur.h) && cur.x >= prev.right() - 0.5 * h;
            let sep = if same_row {
                if cur.x - prev.right() > TAB_GAP * h { "\t" } else { " " }
            } else if cur.y - prev.bottom() > PARAGRAPH_GAP * h || (cur.x - prev.x > INDENT * h && ends_sentence(prev_text)) {
                "\n\n"
            } else {
                "\n"
            };
            out.push_str(sep);
            out.push_str(cur_text);
        }
        out
    }

    /// Cells of the detected table, row-major, or `None` when the layout is
    /// not table-like (fewer than two columns or two rows).
    pub fn table(&self) -> Option<Vec<Vec<String>>> {
        detect_table(self).map(|t| t.rows)
    }

    /// GitHub-flavoured markdown table; the first row becomes the header.
    pub fn to_markdown_table(&self) -> Option<String> {
        let rows = self.table()?;
        let cols = rows[0].len();
        let line = |cells: &[String]| format!("| {} |", cells.iter().map(|c| c.replace('|', r"\|")).collect::<Vec<_>>().join(" | "));
        let mut out = vec![line(&rows[0]), format!("|{}", "---|".repeat(cols))];
        out.extend(rows[1..].iter().map(|r| line(r)));
        Some(out.join("\n"))
    }

    /// Tab-separated values, one table row per line.
    pub fn to_tsv(&self) -> Option<String> {
        let rows = self.table()?;
        Some(
            rows.iter()
                .map(|r| r.iter().map(|c| c.replace(['\t', '\n'], " ")).collect::<Vec<_>>().join("\t"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

fn ends_sentence(s: &str) -> bool {
    s.trim_end().ends_with(['.', '!', '?', ':', '…', '"', '»', '”', ')'])
}

fn median(values: impl Iterator<Item = f32>) -> f32 {
    let mut v: Vec<f32> = values.filter(|x| *x > 0.0).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

fn union(a: RectF, b: RectF) -> RectF {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    RectF::new(x0, y0, a.right().max(b.right()) - x0, a.bottom().max(b.bottom()) - y0)
}

struct Cell<'a> {
    text: &'a str,
    rect: RectF,
}

struct Table {
    rows: Vec<Vec<String>>,
    bounds: RectF,
}

/// Words grouped into visual rows (top to bottom), each sorted left to right,
/// plus the median word height.
fn word_rows(r: &OcrResult) -> (Vec<Vec<Cell<'_>>>, f32) {
    let mut words: Vec<Cell> = Vec::new();
    for line in &r.lines {
        if line.words.is_empty() {
            if !line.text.trim().is_empty() {
                words.push(Cell { text: line.text.trim(), rect: line.rect });
            }
        } else {
            words.extend(line.words.iter().filter(|w| !w.text.trim().is_empty()).map(|w| Cell { text: w.text.trim(), rect: w.rect }));
        }
    }
    let h = median(words.iter().map(|w| w.rect.h)).max(1.0);

    words.sort_by(|a, b| a.rect.center().y.total_cmp(&b.rect.center().y));
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut row_center = 0.0;
    for w in words {
        let cy = w.rect.center().y;
        match rows.last_mut() {
            Some(row) if (cy - row_center).abs() < 0.5 * h.max(w.rect.h) => {
                row.push(w);
                row_center += (cy - row_center) / row.len() as f32;
            }
            _ => {
                row_center = cy;
                rows.push(vec![w]);
            }
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| a.rect.x.total_cmp(&b.rect.x));
    }
    (rows, h)
}

/// The longest run of consecutive rows that share at least one empty vertical
/// strip, so a table inside a screenshot of other text is still found.
fn detect_table(r: &OcrResult) -> Option<Table> {
    let (rows, h) = word_rows(r);
    if rows.len() < 2 {
        return None;
    }
    let min_x = rows.iter().flatten().map(|w| w.rect.x).fold(f32::MAX, f32::min).floor();
    let max_x = rows.iter().flatten().map(|w| w.rect.right()).fold(f32::MIN, f32::max).ceil();
    let width = (max_x - min_x) as usize;
    if width == 0 || width > 100_000 {
        return None;
    }
    let covers: Vec<Vec<bool>> = rows.iter().map(|row| row_cover(row, min_x, width)).collect();

    let mut best: Option<(usize, Table)> = None;
    for start in 0..rows.len() - 1 {
        let mut count = vec![0u32; width + 1];
        for end in start..rows.len() {
            for (c, on) in count.iter_mut().zip(&covers[end]) {
                *c += *on as u32;
            }
            if end == start {
                continue;
            }
            let Some(seps) = separators(&count, end - start + 1, min_x, h) else { break };
            if best.as_ref().is_none_or(|(len, _)| end - start + 1 > *len)
                && let Some(table) = build_table(&rows[start..=end], &seps)
            {
                best = Some((end - start + 1, table));
            }
        }
    }
    best.map(|(_, t)| t)
}

/// Assign words to the columns delimited by `seps`; rejects blocks that are
/// mostly single cells or whose cells read like prose.
fn build_table(block: &[Vec<Cell>], seps: &[(f32, f32)]) -> Option<Table> {
    let columns = seps.len() + 1;
    let table: Vec<Vec<String>> = block
        .iter()
        .map(|row| {
            let mut cells = vec![String::new(); columns];
            for w in row {
                let cx = w.rect.center().x;
                let cell = &mut cells[seps.iter().take_while(|(_, end)| cx > *end).count()];
                if !cell.is_empty() {
                    cell.push(' ');
                }
                cell.push_str(w.text);
            }
            cells
        })
        .collect();

    // First and last rows anchor the table, most rows span several columns and
    // every column carries data in at least half of them.
    let filled = |r: &Vec<String>| r.iter().filter(|c| !c.is_empty()).count();
    let anchored = filled(&table[0]) >= 2 && filled(&table[table.len() - 1]) >= 2;
    let multi = table.iter().filter(|r| filled(r) >= 2).count();
    let columns_used = (0..columns).all(|c| table.iter().filter(|r| !r[c].is_empty()).count() * 2 >= table.len());
    let cells: Vec<&String> = table.iter().flatten().filter(|c| !c.is_empty()).collect();
    let words_per_cell = cells.iter().map(|c| c.split_whitespace().count()).sum::<usize>() as f32 / cells.len().max(1) as f32;
    if !anchored || multi * 10 < table.len() * 7 || !columns_used || words_per_cell > MAX_CELL_WORDS {
        return None;
    }

    let bounds = block.iter().flatten().map(|w| w.rect).reduce(union)?;
    Some(Table { rows: table, bounds })
}

/// Which x positions (relative to `min_x`) a row's words cover.
fn row_cover(row: &[Cell], min_x: f32, width: usize) -> Vec<bool> {
    let mut cover = vec![false; width + 1];
    for w in row {
        let from = ((w.rect.x - min_x).max(0.0) as usize).min(width);
        let to = ((w.rect.right() - min_x).ceil() as usize).min(width);
        cover[from..=to.max(from)].fill(true);
    }
    cover
}

/// Empty vertical strips (start, end) between columns, or `None` for a single column.
fn separators(cover: &[u32], rows: usize, min_x: f32, h: f32) -> Option<Vec<(f32, f32)>> {
    let tolerance = (rows as f32 * CROSSING_ROWS) as u32;
    let first = cover.iter().position(|c| *c > tolerance)?;
    let last = cover.iter().rposition(|c| *c > tolerance)?;
    let min_gap = (COLUMN_GAP * h).max(6.0);

    let mut seps = Vec::new();
    let mut x = first;
    while x <= last {
        if cover[x] > tolerance {
            x += 1;
            continue;
        }
        let start = x;
        while x <= last && cover[x] <= tolerance {
            x += 1;
        }
        if (x - start) as f32 >= min_gap {
            seps.push((min_x + start as f32, min_x + x as f32));
        }
    }
    (!seps.is_empty()).then_some(seps)
}
