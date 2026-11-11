//! Where Yuzora keeps its files. Never the official Herdr app's home: the
//! two apps can run side by side, so Yuzora reads only `YUZORA_APP_HOME`
//! and ignores `HERDR_APP_HOME` (which may point at Herdr's own setup).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Picks another app home (tests, experiments, a second copy).
pub const APP_HOME_ENV: &str = "YUZORA_APP_HOME";
/// The folder under the config root: `~/.config/yuzora`.
pub const APP_DIR_NAME: &str = "yuzora";

/// The person's home folder (the profile folder on Windows).
pub fn home_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| "/".into())
}

/// The app home: `$YUZORA_APP_HOME`, else `$XDG_CONFIG_HOME/yuzora`, else
/// `~/.config/yuzora`, on every OS (like Herdr's own layout).
pub fn app_home() -> PathBuf {
    app_home_from(|k| std::env::var_os(k), home_dir())
}

/// [`app_home`] with the environment and home folder passed in, for tests.
pub fn app_home_from(env: impl Fn(&str) -> Option<OsString>, home: PathBuf) -> PathBuf {
    let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    var(APP_HOME_ENV).unwrap_or_else(|| var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join(APP_DIR_NAME))
}

/// The person's mods (shipped `yuzora-*` mods are installed here too).
pub fn mods_dir() -> PathBuf {
    app_home().join("mods")
}

/// The frame's own mods (`frame-<name>`), rewritten on every start.
pub fn frame_dir() -> PathBuf {
    app_home().join("frame")
}

/// Yuzora's own files inside the app home (settings, caches): `<home>/yuzora`.
pub fn yuzora_dir() -> PathBuf {
    app_home().join("yuzora")
}

/// A mod's folder from its instance name: `frame-x` lives in `frame/x`.
pub fn mod_dir(name: &str) -> PathBuf {
    match name.strip_prefix("frame-") {
        Some(frame) => frame_dir().join(frame),
        None => mods_dir().join(name),
    }
}

/// The `canonicalize` of a Windows path comes back as `\\?\C:\…` (or `\\?\UNC\host\share\…`):
/// what to show people and to compare with the paths other programs give. This is
/// that path in its usual form, `C:\…` (`\\host\share\…`). Left alone when it
/// is not a drive or UNC path, too long (260 characters), or has a name the usual form
/// would read as another (a trailing `.` or space, a device name).
pub fn plain_str(path: &str) -> String {
    const MAX_PATH: usize = 260;
    let Some(rest) = path.strip_prefix(r"\\?\") else { return path.to_owned() };
    let b = rest.as_bytes();
    let plain = if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        rest.to_owned()
    } else if let Some(unc) = rest.strip_prefix(r"UNC\").filter(|u| !u.is_empty()) {
        format!(r"\\{unc}")
    } else {
        return path.to_owned();
    };
    // The usual form re-reads a name that ends in `.` or a space, or is a device name
    // (`CON`, `NUL`, `COM1`…), as another one: those keep the prefix that switches that off.
    let odd = plain.split('\\').any(|c| {
        let stem = c.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.chars().count() == 4 && stem.chars().last().is_some_and(|d| d.is_ascii_digit() || matches!(d, '¹' | '²' | '³')));
        c.ends_with('.') || c.ends_with(' ') || device
    });
    // MAX_PATH counts UTF-16 units, as Windows does, not the bytes of UTF-8 (a Chinese name is 3 bytes, 1 unit).
    if plain.encode_utf16().count() >= MAX_PATH || odd { path.to_owned() } else { plain }
}

/// What Explorer takes to show a file selected: `explorer /select,"C:\a b\c.txt"`, as one
/// raw argument. Explorer wants backslashes, and a space-free `/select, path` pair is
/// not what Rust's quoting of a separate argument gives.
pub fn explorer_select_arg(path: &str) -> String {
    format!("/select,\"{}\"", path.replace('/', "\\"))
}

/// [`plain_str`] for a path; only Windows has these prefixes, so elsewhere it is `path` itself.
pub fn plain(path: &Path) -> PathBuf {
    match path.to_str() {
        Some(s) if cfg!(windows) => PathBuf::from(plain_str(s)),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_gets_one_quoted_backslash_path() {
        assert_eq!(explorer_select_arg("C:/Users/me/a b.txt"), r#"/select,"C:\Users\me\a b.txt""#);
        assert_eq!(explorer_select_arg(r"C:\x\y"), r#"/select,"C:\x\y""#);
    }

    #[test]
    fn canonical_windows_paths_lose_the_verbatim_prefix() {
        assert_eq!(plain_str(r"\\?\C:\Users\me\a b.txt"), r"C:\Users\me\a b.txt");
        assert_eq!(plain_str(r"\\?\c:\"), r"c:\");
        assert_eq!(plain_str(r"\\?\UNC\srv\share\x"), r"\\srv\share\x");
        // Not a drive or UNC path, already plain, or POSIX: unchanged.
        for same in [r"\\?\Volume{1234}\x", r"\\?\C:", r"\\?\UNC\", r"C:\x", r"\\srv\share", "/home/a", ""] {
            assert_eq!(plain_str(same), same);
        }
        // Names the usual form would read as other ones.
        for odd in [r"\\?\C:\data\reports.", r"\\?\C:\data\x \y", r"\\?\C:\NUL", r"\\?\C:\a\com1.txt", r"\\?\UNC\srv\share\lpt9"] {
            assert_eq!(plain_str(odd), odd);
        }
        assert_eq!(plain_str(r"\\?\C:\data\console.txt"), r"C:\data\console.txt");
        // Too long for the usual form to work.
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(plain_str(&long), long);
        // Length is in UTF-16 units: 100 Chinese characters are 300 bytes but 100 units, so they fit;
        // 300 units (ASCII or not) do not. Superscript digits end device names too.
        let zh = format!(r"\\?\C:\{}", "文".repeat(100));
        assert_eq!(plain_str(&zh), zh.strip_prefix(r"\\?\").unwrap());
        let zh_long = format!(r"\\?\C:\{}", "文".repeat(300));
        assert_eq!(plain_str(&zh_long), zh_long);
        assert_eq!(plain_str(r"\\?\C:\a\COM¹.txt"), r"\\?\C:\a\COM¹.txt");
        #[cfg(windows)]
        assert_eq!(plain(Path::new(r"\\?\C:\x")), PathBuf::from(r"C:\x"));
        #[cfg(not(windows))]
        assert_eq!(plain(Path::new(r"\\?\C:\x")), PathBuf::from(r"\\?\C:\x"));
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn defaults_to_dot_config_yuzora() {
        assert_eq!(app_home_from(env(&[]), "/home/p".into()), PathBuf::from("/home/p/.config/yuzora"));
    }

    #[test]
    fn yuzora_env_wins_and_xdg_is_honored() {
        assert_eq!(app_home_from(env(&[("YUZORA_APP_HOME", "/tmp/y"), ("XDG_CONFIG_HOME", "/x")]), "/h".into()), PathBuf::from("/tmp/y"));
        assert_eq!(app_home_from(env(&[("XDG_CONFIG_HOME", "/x")]), "/h".into()), PathBuf::from("/x/yuzora"));
        assert_eq!(app_home_from(env(&[("YUZORA_APP_HOME", "")]), "/h".into()), PathBuf::from("/h/.config/yuzora"));
    }

    #[test]
    fn never_shares_the_official_herdr_home() {
        let home = app_home_from(env(&[("HERDR_APP_HOME", "/h/.config/herdr-app")]), "/h".into());
        assert_eq!(home, PathBuf::from("/h/.config/yuzora"));
        assert_ne!(home.file_name().unwrap(), "herdr-app");
    }
}
