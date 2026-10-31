use std::{fs, path::PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot serialize the config: {0}")]
    Serialize(String),
    #[error("no profile given and no default_profile set")]
    MissingDefaultProfile,
    #[error("unknown profile: {0}")]
    UnknownProfile(String),
    #[error("cannot determine the user config directory")]
    HomeUnavailable,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: std::collections::BTreeMap<String, Profile>,
    // Named filter expressions saved with `:filter-save`; loaded with
    // `:filter-load`. Kept at the top level so they follow the operator
    // across profiles.
    #[serde(default)]
    pub filters: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub host: String,
    pub username: String,
    #[serde(default = "default_true")]
    pub tls: bool,
    #[serde(default = "default_true")]
    pub verify_tls: bool,
    #[serde(default)]
    pub ports: Ports,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ports {
    pub auth: u16,
    pub metadata: u16,
    pub rts: u16,
    pub influx: u16,
}

impl Default for Ports {
    fn default() -> Self {
        Self {
            auth: 8813,
            metadata: 8812,
            rts: 8811,
            influx: 8886,
        }
    }
}

pub fn config_path() -> Result<PathBuf, ConfigError> {
    BaseDirs::new()
        .map(|dirs| dirs.config_dir().join("wntui").join("config.toml"))
        .ok_or(ConfigError::HomeUnavailable)
}

impl Config {
    pub fn from_toml(source: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(source)
    }

    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
        let source = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml(&source).map_err(|error| ConfigError::Parse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
    }

    pub fn resolve_profile(&self, requested: Option<&str>) -> Result<&Profile, ConfigError> {
        let name = requested
            .or(self.default_profile.as_deref())
            .ok_or(ConfigError::MissingDefaultProfile)?;
        self.profiles
            .get(name)
            .ok_or_else(|| ConfigError::UnknownProfile(name.to_owned()))
    }

    // Rewrites the whole config file. Comments do not survive the round
    // trip; keys keep their order (BTreeMap sorts them).
    pub fn save(&self, path: &std::path::Path) -> Result<(), ConfigError> {
        let source = toml::to_string_pretty(self)
            .map_err(|error| ConfigError::Serialize(error.to_string()))?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        fs::write(path, source).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const MINIMAL: &str = r#"
default_profile = "prod"

[profiles.prod]
host = "wnt.example.com"
username = "admin"
"#;

    #[test]
    fn parses_minimal_config_with_port_and_tls_defaults() {
        let config = Config::from_toml(MINIMAL).unwrap();
        let profile = config.resolve_profile(None).unwrap();
        assert_eq!(profile.host, "wnt.example.com");
        assert_eq!(profile.ports, Ports::default());
        assert!(profile.tls);
        assert!(profile.verify_tls);
    }

    #[test]
    fn explicit_ports_override_defaults() {
        let source = r#"
[profiles.lab]
host = "10.0.0.5"
username = "op"
tls = false
verify_tls = false

[profiles.lab.ports]
auth = 9000
metadata = 9001
rts = 9002
influx = 9003
"#;
        let config = Config::from_toml(source).unwrap();
        let profile = config.resolve_profile(Some("lab")).unwrap();
        assert!(!profile.tls);
        assert_eq!(profile.ports.auth, 9000);
        assert_eq!(profile.ports.influx, 9003);
    }

    #[test]
    fn unknown_profile_is_rejected() {
        let config = Config::from_toml(MINIMAL).unwrap();
        assert!(matches!(
            config.resolve_profile(Some("nope")),
            Err(ConfigError::UnknownProfile(_))
        ));
    }

    #[test]
    fn missing_default_profile_is_rejected() {
        let config = Config::from_toml("").unwrap();
        assert!(matches!(
            config.resolve_profile(None),
            Err(ConfigError::MissingDefaultProfile)
        ));
    }

    #[test]
    fn named_filters_parse_from_config_and_round_trip() {
        let source = r#"
default_profile = "prod"

[filters]
offline-lab = "status:offline and fw:<5.2"
sinks = "role:sink"

[profiles.prod]
host = "wnt.example.com"
username = "admin"
"#;
        let config = Config::from_toml(source).unwrap();
        assert_eq!(
            config.filters.get("offline-lab"),
            Some(&"status:offline and fw:<5.2".to_owned())
        );
        assert_eq!(config.filters.get("sinks"), Some(&"role:sink".to_owned()));

        // Round trip: save then load keeps the named filters.
        let directory =
            std::env::temp_dir().join(format!("wntui-config-test-{}", std::process::id()));
        let path = directory.join("config.toml");
        config.save(&path).unwrap();

        let reloaded = Config::load(&path).unwrap();
        assert_eq!(reloaded.filters, config.filters);
        assert_eq!(reloaded.default_profile, config.default_profile);

        std::fs::remove_dir_all(&directory).ok();
    }
}
