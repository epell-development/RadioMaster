//! Translations. Every piece of interface text goes through `tr("English text")`; a language file
//! maps the English text to the translation. Missing entries fall back to English, so a partial
//! translation is fine.
//!
//! Language files are JSON objects (`{ "File": "Datei", … }`) named after the language, e.g.
//! `Deutsch.json`, found in the `lang` folder next to RadioMaster.exe or in the data folder.
//! `lang/template.json` lists every text that can be translated.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, RwLock};

struct State {
    /// English → translation (leaked so `tr` can hand out `&'static str`)
    map: HashMap<&'static str, &'static str>,
    language: String,
}

fn state() -> &'static RwLock<State> {
    static S: OnceLock<RwLock<State>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(State { map: HashMap::new(), language: String::new() }))
}

/// Texts the interface asked for this session (for "Export translation template").
fn seen() -> &'static Mutex<BTreeSet<&'static str>> {
    static S: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// The interface text in the chosen language.
pub fn tr(english: &'static str) -> &'static str {
    let s = state().read().unwrap();
    if s.language.is_empty() {
        return english;
    }
    if let Some(t) = s.map.get(english) {
        return t;
    }
    drop(s);
    if let Ok(mut seen) = seen().lock() {
        seen.insert(english);
    }
    english
}

/// Folders that may hold language files: next to the program, then the data folder.
pub fn folders() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
        v.push(exe_dir.join("lang"));
        // RadioMaster.app keeps them in Contents/Resources (code signing allows only programs in Contents/MacOS)
        if cfg!(target_os = "macos") {
            v.push(exe_dir.join("../Resources/lang"));
        }
    }
    v.push(PathBuf::from("lang")); // running from the source folder
    v.push(crate::config::data_dir().join("lang"));
    v
}

/// Languages that have a file (by file name, without ".json"). English is always available.
pub fn languages() -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for dir in folders() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "json") {
                    let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    if name != "template" && !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// Switch language ("" = English). Returns how many texts the file translates.
pub fn load(language: &str) -> Result<usize, String> {
    let mut map: HashMap<&'static str, &'static str> = HashMap::new();
    if !language.is_empty() {
        let file = folders().into_iter().map(|d| d.join(format!("{language}.json"))).find(|p| p.exists());
        let Some(file) = file else { return Err(format!("no file for {language}")) };
        let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
        let raw: HashMap<String, String> = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        for (k, v) in raw {
            if !v.trim().is_empty() {
                map.insert(Box::leak(k.into_boxed_str()), Box::leak(v.into_boxed_str()));
            }
        }
    }
    let n = map.len();
    let mut s = state().write().unwrap();
    s.map = map;
    s.language = language.to_string();
    Ok(n)
}

/// Write a template with every text (from the shipped template plus anything seen while running)
/// and, when a language is chosen, its current translations — ready to fill in.
pub fn export_template(language: &str) -> Result<PathBuf, String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for dir in folders() {
        if let Ok(t) = std::fs::read_to_string(dir.join("template.json")) {
            if let Ok(m) = serde_json::from_str::<HashMap<String, String>>(&t) {
                keys.extend(m.into_keys());
            }
        }
    }
    keys.extend(seen().lock().map(|s| s.iter().map(|k| k.to_string()).collect::<Vec<_>>()).unwrap_or_default());
    let current = state().read().unwrap();
    let mut out = serde_json::Map::new();
    for k in keys {
        let v = current.map.get(k.as_str()).map(|v| v.to_string()).unwrap_or_default();
        out.insert(k, serde_json::Value::String(v));
    }
    drop(current);
    let dir = crate::config::data_dir().join("lang");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = if language.is_empty() { "new-language".to_string() } else { language.to_string() };
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(path)
}
