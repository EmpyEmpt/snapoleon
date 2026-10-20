#!/usr/bin/env python3
"""Render the package-manager manifests with the real version, URLs and hashes.

Replaces ${{ TOKEN }} placeholders in packaging/{winget,scoop,chocolatey,homebrew} and the
Flatpak manifest, and patches the AUR PKGBUILD. Tokens:

  VERSION, RELEASE_DATE, BASE_URL            derived from --version / --repo
  SHA256_<KEY>                               looked up in SHA256SUMS by release file name

Usage:
  render-manifests.py --version 0.1.0 --sums dist/SHA256SUMS --repo EmpyEmpt/snapoleon --out rendered
"""
import argparse
import datetime
import pathlib
import re
import shutil
import sys

ROOT = pathlib.Path(__file__).resolve().parent

# token key -> release file name pattern ({v} = version)
FILES = {
    "WINDOWS_X64_SETUP": "Snapoleon-{v}-windows-x64-setup.exe",
    "WINDOWS_ARM64_SETUP": "Snapoleon-{v}-windows-arm64-setup.exe",
    "WINDOWS_X64_MSI": "Snapoleon-{v}-windows-x64.msi",
    "WINDOWS_X64_PORTABLE": "Snapoleon-{v}-windows-x64-portable.zip",
    "WINDOWS_ARM64_PORTABLE": "Snapoleon-{v}-windows-arm64-portable.zip",
    "MACOS_UNIVERSAL_DMG": "Snapoleon-{v}-macos-universal.dmg",
    "LINUX_X86_64_TARBALL": "Snapoleon-{v}-linux-x86_64.tar.gz",
    "LINUX_AARCH64_TARBALL": "Snapoleon-{v}-linux-aarch64.tar.gz",
}

SOURCES = [
    ("winget", "winget"),
    ("scoop", "scoop"),
    ("chocolatey", "chocolatey"),
    ("homebrew", "homebrew"),
    ("linux/flatpak", "flatpak"),
]


def read_sums(path):
    sums = {}
    for line in pathlib.Path(path).read_text(encoding="utf-8").splitlines():
        parts = line.split(None, 1)
        if len(parts) == 2:
            sums[parts[1].strip().lstrip("*")] = parts[0]
    return sums


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--sums", required=True)
    ap.add_argument("--repo", default="EmpyEmpt/snapoleon")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    v = a.version.lstrip("v")
    sums = read_sums(a.sums)
    tokens = {
        "VERSION": v,
        "RELEASE_DATE": datetime.date.today().isoformat(),
        "BASE_URL": f"https://github.com/{a.repo}/releases/download/v{v}",
    }
    missing = []
    for key, pattern in FILES.items():
        name = pattern.format(v=v)
        if name in sums:
            tokens[f"SHA256_{key}"] = sums[name]
        else:
            missing.append(name)
            tokens[f"SHA256_{key}"] = "0" * 64

    pat = re.compile(r"\$\{\{\s*([A-Z0-9_]+)\s*\}\}")
    unknown = set()

    def sub(m):
        k = m.group(1)
        if k not in tokens:
            unknown.add(k)
            return m.group(0)
        return tokens[k]

    out = pathlib.Path(a.out)
    if out.exists():
        shutil.rmtree(out)
    for src, dst in SOURCES:
        for f in (ROOT / src).rglob("*"):
            if not f.is_file():
                continue
            rel = f.relative_to(ROOT / src)
            # Only text manifests live here; flatpak dir holds just the yml.
            target = out / dst / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(pat.sub(sub, f.read_text(encoding="utf-8")), encoding="utf-8", newline="\n")

    # AUR: plain PKGBUILD without tokens; patch pkgver and the two checksums.
    pkg = (ROOT / "linux/aur/PKGBUILD").read_text(encoding="utf-8")
    pkg = re.sub(r"^pkgver=.*$", f"pkgver={v}", pkg, flags=re.M)
    pkg = re.sub(r"^pkgrel=.*$", "pkgrel=1", pkg, flags=re.M)
    pkg = re.sub(r"^sha256sums_x86_64=\(.*\)", f"sha256sums_x86_64=('{tokens['SHA256_LINUX_X86_64_TARBALL']}')", pkg, flags=re.M)
    pkg = re.sub(r"^sha256sums_aarch64=\(.*\)", f"sha256sums_aarch64=('{tokens['SHA256_LINUX_AARCH64_TARBALL']}')", pkg, flags=re.M)
    (out / "aur").mkdir(parents=True, exist_ok=True)
    (out / "aur" / "PKGBUILD").write_text(pkg, encoding="utf-8", newline="\n")

    if missing:
        print("warning: no checksum for: " + ", ".join(missing), file=sys.stderr)
    if unknown:
        print("error: unknown tokens: " + ", ".join(sorted(unknown)), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
