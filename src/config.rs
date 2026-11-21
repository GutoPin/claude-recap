//! The optional JSON file that changes the colours.

use std::path::{Path, PathBuf};

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

/// One colour per role, as "#rrggbb". `background` may also be "terminal",
/// which keeps the terminal's own background.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub background: String,
    pub surface: String,
    pub raised: String,
    pub shadow: String,
    pub text: String,
    pub muted: String,
    pub faint: String,
    pub accent: String,
    pub success: String,
    pub warning: String,
    pub danger: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: Theme,
}

const TERMINAL: &str = "terminal";

impl Default for Theme {
    fn default() -> Self {
        let s = String::from;
        Theme {
            background: s("#0b0e14"),
            surface: s("#131822"),
            raised: s("#1e2636"),
            shadow: s("#05070a"),
            text: s("#e6edf3"),
            muted: s("#7d8590"),
            faint: s("#3d444d"),
            accent: s("#d97757"),
            success: s("#56d364"),
            warning: s("#e3b341"),
            danger: s("#ff7b72"),
        }
    }
}

/// Where the file is looked for: `$RECAP_CONFIG`, then
/// `$XDG_CONFIG_HOME/claude-recap/config.json`, then `~/.config/claude-recap/config.json`.
pub fn path() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(file) = var("RECAP_CONFIG") {
        return Some(file.into());
    }
    let base = match var("XDG_CONFIG_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(var("HOME")?).join(".config"),
    };
    Some(base.join("claude-recap").join("config.json"))
}

/// Reads `path` over the defaults. A missing file is fine; a file with a typo
/// is an error, because a setting that silently does nothing is worse.
pub fn load(path: &Path) -> Result<Config, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let config: Config =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    config
        .validate()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(config)
}

fn rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(digits, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

impl Config {
    fn named(&self) -> Vec<(String, &str)> {
        let t = &self.theme;
        let mut all: Vec<(String, &str)> = [
            ("surface", &t.surface),
            ("raised", &t.raised),
            ("shadow", &t.shadow),
            ("text", &t.text),
            ("muted", &t.muted),
            ("faint", &t.faint),
            ("accent", &t.accent),
            ("success", &t.success),
            ("warning", &t.warning),
            ("danger", &t.danger),
        ]
        .into_iter()
        .map(|(key, value)| (format!("theme.{key}"), value.as_str()))
        .collect();
        if t.background != TERMINAL {
            all.push(("theme.background".into(), &t.background));
        }
        all
    }

    fn validate(&self) -> Result<(), String> {
        match self.named().into_iter().find(|(_, v)| rgb(v).is_none()) {
            Some((key, value)) => Err(format!("{key}: {value:?} is not a colour like #1a2b3c")),
            None => Ok(()),
        }
    }

    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }

    /// Resolves the colours for drawing. Without truecolor support each one
    /// falls back to the nearest of the 256 indexed colours.
    pub fn look(&self, truecolor: bool) -> Look {
        let c = |hex: &String| {
            let (r, g, b) = rgb(hex).unwrap_or((0, 0, 0));
            if truecolor {
                Color::Rgb(r, g, b)
            } else {
                Color::Indexed(indexed(r, g, b))
            }
        };
        let t = &self.theme;
        let floats = t.background != TERMINAL;
        Look {
            background: if floats {
                c(&t.background)
            } else {
                Color::Reset
            },
            surface: c(&t.surface),
            raised: c(&t.raised),
            shadow: c(&t.shadow),
            text: c(&t.text),
            muted: c(&t.muted),
            faint: c(&t.faint),
            accent: c(&t.accent),
            floats,
        }
    }
}

/// Whether the terminal says it can show 24-bit colour.
pub fn truecolor() -> bool {
    let var = |name| std::env::var(name).unwrap_or_default();
    let colorterm = var("COLORTERM");
    colorterm.contains("truecolor")
        || colorterm.contains("24bit")
        || !var("WT_SESSION").is_empty()
        || ["iTerm.app", "WezTerm", "vscode", "ghostty"].contains(&var("TERM_PROGRAM").as_str())
}

fn indexed(r: u8, g: u8, b: u8) -> u8 {
    let (min, max) = (r.min(g).min(b), r.max(g).max(b));
    if max - min <= 32 {
        // Near grey: the 24-step ramp is finer than the colour cube.
        let v = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
        return match v {
            0..8 => 16,
            249.. => 231,
            _ => 232 + ((v - 8) / 10).min(23) as u8,
        };
    }
    let cube = |v: u8| match v {
        0..48 => 0,
        48..115 => 1,
        _ => (v - 35) / 40,
    };
    16 + 36 * cube(r) + 6 * cube(g) + cube(b)
}

/// The theme as colours ready to draw with.
#[derive(Clone, Debug)]
pub struct Look {
    pub background: Color,
    pub surface: Color,
    pub raised: Color,
    pub shadow: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    /// False when the terminal's own background is kept: a shadow needs a
    /// known colour underneath to read as one.
    pub floats: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(body: &str) -> (tempdir::Dir, PathBuf) {
        let dir = tempdir::Dir::new();
        let path = dir.path().join("config.json");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    /// A throwaway directory, removed when dropped.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};

        static NEXT: AtomicU32 = AtomicU32::new(0);
        pub struct Dir(PathBuf);
        impl Dir {
            pub fn new() -> Dir {
                let n = NEXT.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("claude-recap-config-{}-{n}", std::process::id()));
                std::fs::create_dir_all(&path).unwrap();
                Dir(path)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempdir::Dir::new();
        assert_eq!(load(&dir.path().join("nope.json")), Ok(Config::default()));
    }

    #[test]
    fn a_file_overrides_only_what_it_names() {
        let (_dir, path) = file(r##"{"theme": {"accent": "#ff00aa", "background": "terminal"}}"##);
        let config = load(&path).unwrap();
        assert_eq!(config.theme.accent, "#ff00aa");
        assert_eq!(config.theme.text, Theme::default().text);
        let look = config.look(true);
        assert_eq!(look.accent, Color::Rgb(255, 0, 170));
        assert_eq!(look.background, Color::Reset);
        assert!(!look.floats);
    }

    #[test]
    fn mistakes_are_reported_with_the_file() {
        for (body, want) in [
            (r##"{"theme": {"acent": "#ffffff"}}"##, "acent"),
            (r##"{"theme": {"accent": "teal"}}"##, "theme.accent"),
            (r##"{"theme": {"surface": "terminal"}}"##, "theme.surface"),
            (r##"{"colours": {}}"##, "colours"),
            (r##"{"theme": "##, "config.json"),
        ] {
            let (_dir, path) = file(body);
            let error = load(&path).unwrap_err();
            assert!(error.contains(want), "{body}: {error}");
            assert!(error.contains("config.json"), "{body}: {error}");
        }
    }

    #[test]
    fn the_defaults_round_trip() {
        let (_dir, path) = file(&Config::default().json());
        assert_eq!(load(&path), Ok(Config::default()));
    }

    #[test]
    fn colours_fall_back_to_the_256_palette() {
        assert_eq!(indexed(0, 0, 0), 16);
        assert_eq!(indexed(255, 255, 255), 231);
        assert_eq!(indexed(255, 0, 0), 196);
        assert_eq!(indexed(0x13, 0x18, 0x22), 233);
        let look = Config::default().look(false);
        assert!(matches!(look.accent, Color::Indexed(_)));
    }
}
