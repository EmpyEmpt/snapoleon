#!/usr/bin/env python3
"""Regenerate the embedded font subsets from the full fonts in assets/fonts/src.

    pip install fonttools brotli
    python tools/subset_fonts.py

Inter and JetBrains Mono keep Latin, Latin-1, Latin Extended, Cyrillic, punctuation, arrows, currency and the
symbols the UI draws as text (modifier keys, check marks, math). Phosphor keeps only the icons that crates/ and
apps/ name (every constant of crates/snap-ui/src/icon.rs that appears anywhere else in the code). Run it again
after using a new icon. The subsets are committed: the build never needs Python.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "assets" / "fonts" / "src"
OUT = ROOT / "assets" / "fonts"
PHOSPHOR_OUT = ROOT / "crates" / "snap-ui" / "assets" / "Phosphor.ttf"

TEXT_UNICODES = ",".join(
    [
        "U+0020-007E",  # Basic Latin
        "U+00A0-00FF",  # Latin-1
        "U+0100-024F",  # Latin Extended-A/B
        "U+0400-052F",  # Cyrillic, Cyrillic Supplement
        "U+1E00-1EFF",  # Latin Extended Additional
        "U+2000-206F",  # General punctuation
        "U+20A0-20CF",  # Currency
        "U+2100-214F",  # Letterlike (TM, No.)
        "U+2190-21FF",  # Arrows
        "U+2200-22FF",  # Math operators
        "U+2300-23FF",  # Technical (command, option, ...)
        "U+25A0-25FF",  # Geometric shapes
        "U+2600-27BF",  # Misc symbols, dingbats (check, cross)
    ]
)
# Kerning and marks are all a plain rasteriser uses; the stylistic sets and contextual forms are dead weight.
FEATURES = "kern,mark,mkmk,ccmp,locl,liga"


def subset(src: Path, dst: Path, args: list[str]) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        sys.executable, "-m", "fontTools.subset", str(src), f"--output-file={dst}",
        "--no-hinting", "--desubroutinize", "--name-IDs=1,2,4,6", "--notdef-outline",
        "--drop-tables+=DSIG,FFTM,STAT,meta,gasp,hdmx,LTSH,VDMX,vhea,vmtx",
        *args,
    ]  # fmt: skip
    subprocess.run(cmd, check=True)
    print(f"{dst.relative_to(ROOT)}: {src.stat().st_size:>7} -> {dst.stat().st_size:>7} bytes")


def used_icon_codepoints() -> list[int]:
    icon_rs = ROOT / "crates" / "snap-ui" / "src" / "icon.rs"
    names = {m[0]: int(m[1], 16) for m in re.findall(r'pub const (\w+): &str = "\\u\{([0-9A-Fa-f]+)\}"', icon_rs.read_text(encoding="utf8"))}
    seen: set[str] = set()
    word = re.compile(r"\b[A-Z][A-Z0-9_]+\b")
    for base in ("crates", "apps"):
        for path in (ROOT / base).rglob("*.rs"):
            if path == icon_rs:
                continue
            seen.update(w for w in word.findall(path.read_text(encoding="utf8", errors="ignore")) if w in names)
    print(f"{len(seen)} of {len(names)} Phosphor icons are used")
    return sorted(names[n] for n in seen)


def main() -> None:
    for name in ("Inter-Regular", "Inter-SemiBold", "Inter-Bold", "JetBrainsMono-Medium"):
        subset(SRC / f"{name}.ttf", OUT / f"{name}.ttf", [f"--unicodes={TEXT_UNICODES}", f"--layout-features={FEATURES}"])
    cps = ",".join(f"U+{c:04X}" for c in used_icon_codepoints())
    subset(SRC / "Phosphor.ttf", PHOSPHOR_OUT, [f"--unicodes={cps}", "--layout-features="])


if __name__ == "__main__":
    main()
