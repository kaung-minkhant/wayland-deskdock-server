//! Capture configuration: which Linux input devices may be forwarded.
//!
//! Only devices listed here are ever opened or grabbed.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const CONFIG_DIRECTORY: &str = "deskdock-server";
pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub tablet: Tablet,
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tablet {
    /// Multiplier applied to relative mouse and touchpad motion.
    #[serde(default = "default_sensitivity")]
    pub sensitivity: i32,
    /// Touchpad travel, as a percent of its height, per forwarded scroll step.
    #[serde(default = "default_touchpad_scroll_percent")]
    pub touchpad_scroll_percent: i32,
    /// Delay a single tap so a following tap-and-hold can become a drag.
    #[serde(default = "default_touchpad_tap_delay_ms")]
    pub touchpad_tap_delay_ms: u64,
}

impl Default for Tablet {
    fn default() -> Self {
        Self {
            sensitivity: default_sensitivity(),
            touchpad_scroll_percent: default_touchpad_scroll_percent(),
            touchpad_tap_delay_ms: default_touchpad_tap_delay_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub path: PathBuf,
    pub kind: DeviceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Mouse,
    Touchpad,
    Keyboard,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        let contents = fs::read_to_string(path)
            .map_err(|error| format!("could not read configuration {}: {error}", path.display()))?;
        Self::parse(&contents, path)
    }

    fn parse(contents: &str, path: &Path) -> Result<Self, Box<dyn Error>> {
        let config: Self = toml::from_str(contents).map_err(|error| {
            format!("could not parse configuration {}: {error}", path.display())
        })?;
        config.validate(path)?;
        Ok(config)
    }

    fn validate(&self, path: &Path) -> Result<(), Box<dyn Error>> {
        let name = path.display();
        if self.tablet.sensitivity <= 0 {
            return Err(format!("{name} must use a positive tablet.sensitivity").into());
        }
        if !(1..=100).contains(&self.tablet.touchpad_scroll_percent) {
            return Err(format!(
                "{name} must use tablet.touchpad_scroll_percent between 1 and 100"
            )
            .into());
        }
        if !(1..=2_000).contains(&self.tablet.touchpad_tap_delay_ms) {
            return Err(
                format!("{name} must use tablet.touchpad_tap_delay_ms between 1 and 2000").into(),
            );
        }
        if self.devices.is_empty() {
            return Err(format!("{name} must configure at least one [[devices]] entry").into());
        }
        for (index, device) in self.devices.iter().enumerate() {
            if !device.path.is_absolute() {
                return Err(format!(
                    "{name} contains a device path that is not absolute: {}",
                    device.path.display()
                )
                .into());
            }
            if self.devices[..index]
                .iter()
                .any(|other| other.path == device.path)
            {
                return Err(
                    format!("{name} configures {} more than once", device.path.display()).into(),
                );
            }
        }
        Ok(())
    }
}

pub fn default_path() -> Result<PathBuf, Box<dyn Error>> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
    Ok(config_home.join(CONFIG_DIRECTORY).join(CONFIG_FILE))
}

fn default_sensitivity() -> i32 {
    3
}

fn default_touchpad_scroll_percent() -> i32 {
    5
}

fn default_touchpad_tap_delay_ms() -> u64 {
    500
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(contents: &str) -> Result<Config, Box<dyn Error>> {
        Config::parse(contents, Path::new("test.toml"))
    }

    #[test]
    fn parses_devices_with_defaults() {
        let config = parse(
            r#"
[[devices]]
path = "/dev/input/by-id/mouse"
kind = "mouse"

[[devices]]
path = "/dev/input/by-id/kbd"
kind = "keyboard"
"#,
        )
        .unwrap();
        assert_eq!(config.devices.len(), 2);
        assert_eq!(config.devices[1].kind, DeviceKind::Keyboard);
        assert_eq!(config.tablet.sensitivity, 3);
    }

    #[test]
    fn rejects_relative_duplicate_and_empty_devices() {
        assert!(parse("devices = []").is_err());
        assert!(parse("[[devices]]\npath = \"mouse\"\nkind = \"mouse\"\n").is_err());
        assert!(
            parse(
                "[[devices]]\npath = \"/a\"\nkind = \"mouse\"\n[[devices]]\npath = \"/a\"\nkind = \"mouse\"\n"
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_unknown_fields_and_bad_sensitivity() {
        assert!(
            parse("[tablet]\nbogus = 1\n[[devices]]\npath = \"/a\"\nkind = \"mouse\"\n").is_err()
        );
        assert!(
            parse("[tablet]\nsensitivity = 0\n[[devices]]\npath = \"/a\"\nkind = \"mouse\"\n")
                .is_err()
        );
    }
}
