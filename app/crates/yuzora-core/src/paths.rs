//! Where Yuzora keeps its files. Never the official Herdr app's home: the
//! two apps can run side by side, so Yuzora reads only `YUZORA_APP_HOME`
//! and ignores `HERDR_APP_HOME` (which may point at Herdr's own setup).

use std::ffi::OsString;
use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

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
