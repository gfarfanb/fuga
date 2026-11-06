//! Configuration (§11): platform paths, `config.toml`, and source-name validation.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Windows device names that a `<name>.db` index file would inherit.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters Windows forbids in a filename plus the wikilink delimiters `[]#`.
const FORBIDDEN_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*', '[', ']', '#'];

/// The three persistent paths are derived from the config and data dirs and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn new() -> Result<Self, ConfigError> {
        let dirs = ProjectDirs::from("", "", "fuga").ok_or(ConfigError::NoProjectDirs)?;
        Ok(Self {
            config_dir: dirs.config_dir().to_path_buf(),
            data_dir: dirs.data_dir().to_path_buf(),
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn index_dir(&self) -> PathBuf {
        self.data_dir.join("index")
    }

    pub fn index_file(&self, source: &str) -> PathBuf {
        self.index_dir().join(format!("{source}.db"))
    }

    pub fn session_file(&self) -> PathBuf {
        self.data_dir.join("session.json")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no config directory for this platform")]
    NoProjectDirs,
    #[error("could not read config: {0}")]
    Read(#[source] std::io::Error),
    #[error("invalid config: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("source name {0:?} is invalid: {1}")]
    InvalidSourceName(String, &'static str),
    #[error("duplicate source name {0:?}")]
    DuplicateSourceName(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub sources: Vec<SourceConfig>,
    pub server: ServerConfig,
    pub client: ClientConfig,
    pub editor: EditorConfig,
    pub index: IndexConfig,
    pub ui: UiConfig,
    /// Action id (§10.3) to key string; unlisted actions keep their default.
    pub keymap: BTreeMap<String, String>,
}

impl Config {
    /// Read and validate `config.toml`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(ConfigError::Read)?;
        Self::from_toml(&text)
    }

    /// Parse and validate config text. Source names are checked here, before any
    /// path is derived from them, so a bad name can never escape `<index dir>`.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        for (i, source) in self.sources.iter().enumerate() {
            validate_source_name(&source.name)
                .map_err(|reason| ConfigError::InvalidSourceName(source.name.clone(), reason))?;
            if self.sources[..i].iter().any(|s| s.name == source.name) {
                return Err(ConfigError::DuplicateSourceName(source.name.clone()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SourceConfig {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub capture_dir: Option<String>,
    #[serde(default)]
    pub auto_write_id: bool,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub watch_mode: WatchMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchMode {
    #[default]
    Auto,
    Native,
    Poll,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerConfig {
    pub transport: Transport,
    /// Empty means the platform default socket path (§5.3).
    pub socket: String,
    pub tcp_addr: String,
    pub token: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            transport: Transport::Local,
            socket: String::new(),
            tcp_addr: "127.0.0.1:7737".into(),
            token: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Local,
    Tcp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ClientConfig {
    /// Empty means all enabled sources.
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Empty means `$EDITOR`.
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct IndexConfig {
    pub watch: bool,
    pub poll_interval_ms: u64,
    /// Empty means the default data dir.
    pub index_dir: String,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            watch: true,
            poll_interval_ms: 2000,
            index_dir: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: String,
    /// Display only; never contributes to a note id, filename, or path (§6.2).
    pub date_format: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            date_format: "%Y-%m-%d".into(),
        }
    }
}

/// A source name is used verbatim as `<name>.db`, so it must not be empty, a path
/// component, a Windows device name, or contain a filename/wikilink delimiter.
pub fn validate_source_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("empty");
    }
    if name == "." || name == ".." {
        return Err("path component");
    }
    if name.contains(['/', '\\']) {
        return Err("path separator");
    }
    if name.contains(FORBIDDEN_CHARS) {
        return Err("forbidden character");
    }
    // Windows reserves the device name even with an extension, and `<name>.db` adds one.
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return Err("Windows-reserved device name");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_config_parses() {
        let config = Config::from_toml(
            r#"
            [[sources]]
            name = "notes"
            path = "~/notes"
            capture_dir = "inbox"
            auto_write_id = true
            exclude = ["archive/**", "templates/**"]
            watch_mode = "auto"

            [[sources]]
            name = "work"
            path = "~/work/docs"
            capture_dir = "inbox"
            auto_write_id = false
            exclude = ["drafts/**"]

            [server]
            transport = "local"
            socket = ""
            tcp_addr = "127.0.0.1:7737"
            token = ""

            [client]
            source = ""

            [editor]
            command = ""

            [index]
            watch = true
            poll_interval_ms = 2000
            index_dir = ""

            [ui]
            theme = "dark"
            date_format = "%Y-%m-%d"

            [keymap]
            edit_external = "ctrl-e"
            "#,
        )
        .unwrap();

        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.sources[0].name, "notes");
        assert!(config.sources[0].auto_write_id);
        assert_eq!(config.sources[0].watch_mode, WatchMode::Auto);
        assert_eq!(config.sources[1].watch_mode, WatchMode::Auto);
        assert_eq!(config.server.tcp_addr, "127.0.0.1:7737");
        assert_eq!(config.index.poll_interval_ms, 2000);
        assert_eq!(config.ui.date_format, "%Y-%m-%d");
        assert_eq!(config.keymap["edit_external"], "ctrl-e");
    }

    #[test]
    fn defaults_are_applied() {
        let config =
            Config::from_toml("[[sources]]\nname = \"notes\"\npath = \"/tmp/notes\"\n").unwrap();
        assert_eq!(config.sources[0].capture_dir, None);
        assert!(!config.sources[0].auto_write_id);
        assert!(config.sources[0].exclude.is_empty());
        assert_eq!(config.sources[0].watch_mode, WatchMode::Auto);
        assert!(config.index.watch);
        assert_eq!(config.index.poll_interval_ms, 2000);
        assert_eq!(config.ui.theme, "dark");
        assert_eq!(config.server.transport, Transport::Local);
    }

    #[test]
    fn invalid_source_names_are_rejected() {
        let bad = [
            ("", "empty"),
            (".", "path component"),
            ("..", "path component"),
            ("a/b", "path separator"),
            ("a\\b", "path separator"),
            ("notes[1]", "forbidden character"),
            ("no#tes", "forbidden character"),
            ("a:b", "forbidden character"),
            ("CON", "Windows-reserved device name"),
            ("nul", "Windows-reserved device name"),
            ("com1", "Windows-reserved device name"),
            ("NUL.md", "Windows-reserved device name"),
        ];
        for (name, reason) in bad {
            assert_eq!(validate_source_name(name), Err(reason), "name {name:?}");
        }
    }

    #[test]
    fn valid_source_names_are_accepted() {
        for name in ["notes", "work-docs", "notes.v1", "inbox_2", "Ünïcode"] {
            assert_eq!(validate_source_name(name), Ok(()), "name {name:?}");
        }
    }

    #[test]
    fn duplicate_source_names_are_rejected() {
        let err = Config::from_toml(
            "[[sources]]\nname = \"notes\"\npath = \"/a\"\n\
             [[sources]]\nname = \"notes\"\npath = \"/b\"\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateSourceName(name) if name == "notes"));
    }

    #[test]
    fn invalid_name_is_rejected_before_paths_are_derived() {
        let err =
            Config::from_toml("[[sources]]\nname = \"../escape\"\npath = \"/a\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::InvalidSourceName(..)));
    }

    #[test]
    fn paths_derive_index_and_session_from_the_data_dir() {
        let paths = Paths {
            config_dir: PathBuf::from("/cfg"),
            data_dir: PathBuf::from("/data"),
        };
        assert_eq!(paths.config_file(), PathBuf::from("/cfg/config.toml"));
        assert_eq!(
            paths.index_file("notes"),
            PathBuf::from("/data/index/notes.db")
        );
        assert_eq!(paths.session_file(), PathBuf::from("/data/session.json"));
    }
}
