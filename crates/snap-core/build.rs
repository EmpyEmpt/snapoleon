//! Embeds every `locales/<lang>/*.toml` file at compile time so translators
//! (and parallel feature work) can add files without touching a shared index.

use std::{env, fs, path::Path};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales");
    println!("cargo:rerun-if-changed={}", root.display());

    let mut entries = Vec::new();
    if let Ok(langs) = fs::read_dir(&root) {
        for lang in langs.flatten().filter(|e| e.path().is_dir()) {
            let lang_name = lang.file_name().to_string_lossy().into_owned();
            println!("cargo:rerun-if-changed={}", lang.path().display());
            let mut files: Vec<_> = fs::read_dir(lang.path())
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect();
            files.sort();
            for file in files {
                println!("cargo:rerun-if-changed={}", file.display());
                let abs = file.canonicalize().unwrap().display().to_string().replace('\\', "/");
                let abs = abs.trim_start_matches("//?/").to_string();
                entries.push(format!("({lang_name:?}, include_str!({abs:?})),"));
            }
        }
    }

    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("locales.rs");
    fs::write(
        out,
        format!("pub(crate) static EMBEDDED: &[(&str, &str)] = &[\n{}\n];\n", entries.join("\n")),
    )
    .unwrap();
}
