//! The single source of truth for every path this application owns.
//!
//! All runtime data (config files, the working database, attachments, logs and
//! sync state) lives under one application-specific data directory. Business
//! code must never hard-code `%APPDATA%`, `$HOME` or another application's
//! profile path; derive paths from here instead.
//!
//! The root can be overridden with `JOPLIN_RS_DATA_DIR`, which makes portable
//! installs and isolated tests possible without touching the real profile.

use std::{
    env,
    fs,
    io,
    path::{Path, PathBuf},
};

pub const DATA_DIR_ENV: &str = "JOPLIN_RS_DATA_DIR";

/// Per-platform, per-application root. Never points at another app's folder.
fn platform_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("joplin-rs")
    }
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library")
            .join("Application Support")
            .join("joplin-rs")
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let base = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".local")
                    .join("share")
            });
        base.join("joplin-rs")
    }
}

pub fn data_dir() -> PathBuf {
    env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(platform_data_dir)
}

/// Everything the app stores, derived from one root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLayout {
    pub root: PathBuf,
    pub profile_config: PathBuf,
    pub sync_config: PathBuf,
    pub database: PathBuf,
    pub resources: PathBuf,
    pub log: PathBuf,
}

pub fn layout(root: &Path) -> DataLayout {
    DataLayout {
        root: root.to_path_buf(),
        profile_config: root.join("profile.json"),
        sync_config: root.join("sync.json"),
        database: root.join("notes.sqlite"),
        resources: root.join("resources"),
        log: root.join("startup.log"),
    }
}

pub fn paths() -> DataLayout {
    layout(&data_dir())
}

/// Create the data directory (and subdirectories) once at startup.
pub fn ensure() -> io::Result<DataLayout> {
    let p = paths();
    fs::create_dir_all(&p.resources)?;
    Ok(p)
}

pub fn resources_dir() -> PathBuf {
    paths().resources
}

pub fn log_path() -> PathBuf {
    paths().log
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_is_inside_the_app_data_dir() {
        let p = layout(Path::new("C:/Users/test/AppData/Roaming/joplin-rs"));
        assert_eq!(p.root, PathBuf::from("C:/Users/test/AppData/Roaming/joplin-rs"));
        assert_eq!(p.profile_config, p.root.join("profile.json"));
        assert_eq!(p.sync_config, p.root.join("sync.json"));
        assert_eq!(p.database, p.root.join("notes.sqlite"));
        assert_eq!(p.resources, p.root.join("resources"));
        assert_eq!(p.log, p.root.join("startup.log"));
        for child in [
            &p.profile_config,
            &p.sync_config,
            &p.database,
            &p.resources,
            &p.log,
        ] {
            assert!(child.starts_with(&p.root), "{} escapes the data dir", child.display());
        }
    }

    #[test]
    fn layout_is_pure_and_deterministic() {
        let a = layout(Path::new("/tmp/app-a"));
        let b = layout(Path::new("/tmp/app-b"));
        assert_eq!(a, layout(Path::new("/tmp/app-a")));
        assert_ne!(a.database, b.database);
        assert_eq!(a.resources.file_name().unwrap(), "resources");
    }
}
