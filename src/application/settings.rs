//! Typed settings persistence and snapshot use cases.
//!
//! The portable SQLite database is the sole persistence dependency. Runtime
//! effects such as restarting streams or changing the log subscriber remain
//! with the desktop coordinator and happen only after these operations commit.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::application::sidecar_policy::SidecarPolicy;
use crate::db::pool::Database;
use crate::db::queries::settings;
use crate::state::account_source_color::AccountSourceColor;
use crate::state::appearance::AppearanceSettings;
use crate::state::bluesky_fetch::BlueskyFetchSettings;
use crate::state::confirmation::ConfirmationSettings;
use crate::state::debug_settings::DebugSettings;
use crate::state::notifications::NotificationSuppressionList;
use crate::state::performance::PerformanceSettings;
use crate::state::preset_visibility::PresetVisibilitySettings;

const SIDECAR_MIN_WIDTH: u32 = 160;
const SIDECAR_DEFAULT_WIDTH: u32 = 500;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSnapshot {
    appearance: AppearanceSettings,
    performance: PerformanceSettings,
    confirmation: ConfirmationSettings,
    bluesky_fetch: BlueskyFetchSettings,
    sidecars: SidecarSettings,
    account_source_colors: HashMap<String, AccountSourceColor>,
    preset_visibility: PresetVisibilitySettings,
    debug: DebugSettings,
    notification_suppression: NotificationSuppressionList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarEntry {
    id: String,
    name: String,
    url: String,
    #[serde(default)]
    user_style_enabled: bool,
    #[serde(default)]
    user_style: String,
    width: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SidecarSettings {
    #[serde(default)]
    entries: Vec<SidecarEntry>,
    #[serde(default)]
    main_view_index: usize,
}

impl SidecarSettings {
    fn normalized(self) -> Result<Self, String> {
        let mut entries = Vec::new();
        for entry in self.entries {
            let id = entry.id.trim().to_string();
            let name = entry.name.trim().to_string();
            let url = entry.url.trim().to_string();
            if id.is_empty() {
                return Err("Sidecar id is empty".to_string());
            }
            SidecarPolicy::parse_initial_url(&url)?;
            entries.push(SidecarEntry {
                id,
                name: if name.is_empty() {
                    "Sidecar".to_string()
                } else {
                    name
                },
                url,
                user_style_enabled: entry.user_style_enabled,
                user_style: entry.user_style,
                width: if entry.width == 0 {
                    SIDECAR_DEFAULT_WIDTH
                } else {
                    entry.width.max(SIDECAR_MIN_WIDTH)
                },
            });
        }

        Ok(Self {
            entries,
            main_view_index: 0,
        })
    }
}

pub(crate) fn validated_settings_json(
    key: &str,
    value: serde_json::Value,
) -> Result<String, String> {
    fn encode<T>(value: serde_json::Value) -> Result<String, String>
    where
        T: serde::de::DeserializeOwned + Serialize,
    {
        let typed = serde_json::from_value::<T>(value).map_err(|error| error.to_string())?;
        serde_json::to_string(&typed).map_err(|error| error.to_string())
    }

    let descriptor = crate::ipc::contract::SETTINGS
        .iter()
        .find(|setting| setting.key == key)
        .ok_or_else(|| format!("Unsupported settings key: {key}"))?;
    debug_assert_eq!(
        descriptor.schema_version,
        crate::ipc::contract::SETTINGS_SCHEMA_VERSION
    );
    match descriptor.key {
        "appearance" => encode::<AppearanceSettings>(value),
        "performance" => encode::<PerformanceSettings>(value),
        "confirmation" => encode::<ConfirmationSettings>(value),
        "bluesky_fetch" => {
            let typed = serde_json::from_value::<BlueskyFetchSettings>(value)
                .map_err(|error| error.to_string())?
                .normalized();
            serde_json::to_string(&typed).map_err(|error| error.to_string())
        }
        "sidecars" => {
            let typed = serde_json::from_value::<SidecarSettings>(value)
                .map_err(|error| error.to_string())?
                .normalized()?;
            serde_json::to_string(&typed).map_err(|error| error.to_string())
        }
        "account_source_colors" => encode::<HashMap<String, AccountSourceColor>>(value),
        "preset_visibility" => encode::<PresetVisibilitySettings>(value),
        "debug" => encode::<DebugSettings>(value),
        "notification_suppression" => encode::<NotificationSuppressionList>(value),
        _ => unreachable!("every generated setting descriptor must have a validator"),
    }
}

pub(crate) async fn settings_snapshot(database: &Database) -> Result<SettingsSnapshot, String> {
    Ok(SettingsSnapshot {
        appearance: load_setting(database, "appearance").await?,
        performance: load_setting(database, "performance").await?,
        confirmation: load_setting(database, "confirmation").await?,
        bluesky_fetch: load_setting::<BlueskyFetchSettings>(database, "bluesky_fetch")
            .await?
            .normalized(),
        sidecars: load_setting::<SidecarSettings>(database, "sidecars")
            .await?
            .normalized()?,
        account_source_colors: load_setting(database, "account_source_colors").await?,
        preset_visibility: load_setting(database, "preset_visibility").await?,
        debug: load_setting(database, "debug").await?,
        notification_suppression: load_setting(database, "notification_suppression").await?,
    })
}

pub(crate) async fn load_setting<T>(database: &Database, key: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + Serialize + Default,
{
    match settings::get_setting(database.reader(), key)
        .await
        .map_err(|error| error.to_string())?
    {
        Some(json) => match serde_json::from_str(&json) {
            Ok(value) => Ok(value),
            Err(error) => {
                let default_value = T::default();
                let default_json = serde_json::to_string(&default_value)
                    .map_err(|serialize_error| serialize_error.to_string())?;
                let backup_key = settings::backup_and_reset_corrupt_setting(
                    database.writer(),
                    key,
                    &json,
                    &default_json,
                )
                .await
                .map_err(|backup_error| backup_error.to_string())?;
                tracing::error!(
                    setting = key,
                    backup_key,
                    %error,
                    "Invalid setting was backed up and reset"
                );
                Err(format!(
                    "Stored setting '{key}' was invalid and has been reset; retry the operation"
                ))
            }
        },
        None => Ok(T::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn settings_registry_rejects_unknown_keys_and_invalid_values() {
        assert!(validated_settings_json("unknown", serde_json::json!({})).is_err());
        assert!(validated_settings_json(
            "appearance",
            serde_json::json!({
                "avatar_shape": "Triangle",
                "font_size": "Medium",
                "cw_behavior": "Hide",
                "nsfw_behavior": "Hide",
                "display_mode": "StarryEyes"
            })
        )
        .is_err());
    }

    #[test]
    fn settings_registry_round_trips_every_supported_type() {
        let fixtures = [
            (
                "appearance",
                serde_json::to_value(AppearanceSettings::default()).unwrap(),
            ),
            (
                "performance",
                serde_json::to_value(PerformanceSettings::default()).unwrap(),
            ),
            (
                "confirmation",
                serde_json::to_value(ConfirmationSettings::default()).unwrap(),
            ),
            (
                "bluesky_fetch",
                serde_json::to_value(BlueskyFetchSettings::default()).unwrap(),
            ),
            (
                "sidecars",
                serde_json::to_value(SidecarSettings::default()).unwrap(),
            ),
            ("account_source_colors", serde_json::json!({})),
            (
                "preset_visibility",
                serde_json::to_value(PresetVisibilitySettings::default()).unwrap(),
            ),
            (
                "debug",
                serde_json::to_value(DebugSettings::default()).unwrap(),
            ),
            (
                "notification_suppression",
                serde_json::to_value(NotificationSuppressionList::default()).unwrap(),
            ),
        ];

        let fixture_keys = fixtures.iter().map(|(key, _)| *key).collect::<HashSet<_>>();
        let registry_keys = crate::ipc::contract::SETTINGS
            .iter()
            .map(|setting| setting.key)
            .collect::<HashSet<_>>();
        assert_eq!(registry_keys, fixture_keys);
        assert!(crate::ipc::contract::SETTINGS.iter().all(|setting| {
            setting.schema_version == crate::ipc::contract::SETTINGS_SCHEMA_VERSION
        }));

        for (key, value) in fixtures {
            let json = validated_settings_json(key, value).expect("validate setting");
            assert!(
                serde_json::from_str::<serde_json::Value>(&json).is_ok(),
                "{key}"
            );
        }
    }
}
