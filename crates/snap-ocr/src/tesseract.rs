//! `tesseract` command-line engine (TSV output).

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use image::{DynamicImage, ImageFormat};
use snap_core::settings::OcrSettings;
use snap_core::{Error, RectF, Result, RgbaImage, t, tf};

use crate::{OcrLine, OcrResult, OcrWord};

/// BCP-47 primary subtag to tesseract language pack.
const LANG_CODES: &[(&str, &str)] = &[
    ("ar", "ara"), ("bg", "bul"), ("cs", "ces"), ("da", "dan"), ("de", "deu"), ("el", "ell"), ("en", "eng"), ("es", "spa"),
    ("fa", "fas"), ("fi", "fin"), ("fr", "fra"), ("he", "heb"), ("hi", "hin"), ("hr", "hrv"), ("hu", "hun"), ("id", "ind"),
    ("it", "ita"), ("ja", "jpn"), ("ko", "kor"), ("nl", "nld"), ("no", "nor"), ("nb", "nor"), ("pl", "pol"), ("pt", "por"),
    ("ro", "ron"), ("ru", "rus"), ("sk", "slk"), ("sr", "srp"), ("sv", "swe"), ("th", "tha"), ("tr", "tur"), ("uk", "ukr"),
    ("vi", "vie"), ("zh", "chi_sim"),
];

/// Languages used when settings name none (and the packs are installed).
const DEFAULT_LANGS: &[&str] = &["eng", "rus"];

fn command(exe: &Path) -> Command {
    no_window(Command::new(exe))
}

#[cfg(windows)]
fn no_window(mut c: Command) -> Command {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

#[cfg(not(windows))]
fn no_window(c: Command) -> Command {
    c
}

/// Configured path, else `tesseract` on PATH (plus the usual Windows install dir).
fn locate(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        return if p.is_dir() { find_in(std::iter::once(p)) } else { p.is_file().then_some(p) };
    }
    let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    #[cfg(windows)]
    let path = {
        let mut path = path;
        path.push(PathBuf::from(r"C:\Program Files\Tesseract-OCR"));
        path
    };
    find_in(path.into_iter())
}

fn find_in(dirs: impl Iterator<Item = PathBuf>) -> Option<PathBuf> {
    let name = if cfg!(windows) { "tesseract.exe" } else { "tesseract" };
    dirs.map(|d| d.join(name)).find(|p| p.is_file())
}

pub(crate) fn installed_languages(configured: &str) -> Vec<String> {
    let Some(exe) = locate(configured) else { return Vec::new() };
    let Ok(out) = command(&exe).arg("--list-langs").output() else { return Vec::new() };
    // Older builds print the list on stderr.
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    text.lines().skip_while(|l| !l.starts_with("List of")).skip(1).map(str::trim).filter(|l| !l.is_empty() && *l != "osd").map(String::from).collect()
}

pub(crate) fn status(configured: &str) -> (bool, String) {
    match locate(configured) {
        Some(p) => (true, tf("ocr.note.tesseract_found", &[("path", &p.display().to_string())])),
        None => (false, t("ocr.note.tesseract_missing").into_owned()),
    }
}

/// Pack list for `-l`: requested languages that are installed, else the defaults that are.
fn pack_list(wanted: &[String], installed: &[String]) -> Vec<String> {
    let mut packs: Vec<String> = Vec::new();
    for w in wanted {
        let primary = w.split('-').next().unwrap_or(w).to_ascii_lowercase();
        let code = LANG_CODES.iter().find(|(p, _)| *p == primary).map(|(_, c)| c.to_string()).unwrap_or(primary);
        if installed.contains(&code) && !packs.contains(&code) {
            packs.push(code);
        }
    }
    if packs.is_empty() {
        packs = DEFAULT_LANGS.iter().filter(|c| installed.iter().any(|i| i == *c)).map(|c| c.to_string()).collect();
    }
    packs
}

pub(crate) fn recognize(img: &RgbaImage, s: &OcrSettings) -> Result<OcrResult> {
    let exe = locate(&s.tesseract_path).ok_or_else(|| Error::NotFound(t("ocr.note.tesseract_missing").into_owned()))?;
    let packs = pack_list(&s.languages, &installed_languages(&s.tesseract_path));

    let mut png = Vec::new();
    DynamicImage::ImageRgba8(flatten(img)).to_rgb8().write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;

    let mut cmd = command(&exe);
    cmd.args(["stdin", "stdout"]);
    if !packs.is_empty() {
        cmd.arg("-l").arg(packs.join("+"));
    }
    cmd.arg("tsv").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    // Feed from a thread: tesseract may start writing before it has read everything.
    let writer = std::thread::spawn(move || stdin.write_all(&png));
    let out = child.wait_with_output()?;
    let _ = writer.join();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Other(anyhow::anyhow!("tesseract failed ({}): {}", out.status, err.trim())));
    }

    Ok(OcrResult {
        lines: parse_tsv(&String::from_utf8_lossy(&out.stdout)),
        language: (!packs.is_empty()).then(|| packs.join("+")),
        engine: "tesseract".into(),
    })
}

/// Composite onto white so transparent pixels do not become black.
fn flatten(img: &RgbaImage) -> RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let a = p[3] as u32;
        for c in &mut p.0[..3] {
            *c = ((*c as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
        p[3] = 255;
    }
    out
}

/// Word rows (level 5) grouped by block/paragraph/line.
fn parse_tsv(tsv: &str) -> Vec<OcrLine> {
    let mut lines: Vec<OcrLine> = Vec::new();
    let mut current: Option<(u32, u32, u32)> = None;
    for row in tsv.lines().skip(1) {
        let f: Vec<&str> = row.splitn(12, '\t').collect();
        if f.len() < 12 || f[0] != "5" {
            continue;
        }
        let num = |i: usize| f[i].trim().parse::<f32>().ok();
        let (Some(block), Some(par), Some(line), Some(x), Some(y), Some(w), Some(h), Some(conf)) =
            (num(2), num(3), num(4), num(6), num(7), num(8), num(9), num(10))
        else {
            continue;
        };
        let text = f[11].trim();
        if text.is_empty() || conf < 0.0 {
            continue;
        }
        let key = (block as u32, par as u32, line as u32);
        let word = OcrWord { text: text.to_string(), rect: RectF::new(x, y, w, h), confidence: (conf / 100.0).clamp(0.0, 1.0) };
        match lines.last_mut() {
            Some(l) if current == Some(key) => {
                l.text.push(' ');
                l.text.push_str(&word.text);
                let (x0, y0) = (l.rect.x.min(word.rect.x), l.rect.y.min(word.rect.y));
                l.rect = RectF::new(x0, y0, l.rect.right().max(word.rect.right()) - x0, l.rect.bottom().max(word.rect.bottom()) - y0);
                l.words.push(word);
            }
            _ => {
                current = Some(key);
                lines.push(OcrLine { text: word.text.clone(), rect: word.rect, words: vec![word] });
            }
        }
    }
    lines
}
