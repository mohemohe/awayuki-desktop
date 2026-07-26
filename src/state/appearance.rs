use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AvatarShape {
    Square,
    Circle,
    Rounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontSize {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CwBehavior {
    Hide,
    AlwaysExpand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NsfwBehavior {
    Hide,
    AlwaysShow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    Latte,
    Frappe,
    Macchiato,
    #[default]
    Mocha,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DisplayMode {
    #[default]
    StarryEyes,
    Mystique,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub avatar_shape: AvatarShape,
    pub font_size: FontSize,
    pub cw_behavior: CwBehavior,
    pub nsfw_behavior: NsfwBehavior,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub display_mode: DisplayMode,
    #[serde(default)]
    pub visibility_background_enabled: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            avatar_shape: AvatarShape::Circle,
            font_size: FontSize::Medium,
            cw_behavior: CwBehavior::Hide,
            nsfw_behavior: NsfwBehavior::Hide,
            theme: Theme::Mocha,
            display_mode: DisplayMode::StarryEyes,
            visibility_background_enabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppearanceSettings, Theme};

    #[test]
    fn defaults_new_fields_for_existing_settings() {
        let settings: AppearanceSettings = serde_json::from_str(
            r#"{
                "avatar_shape":"Circle",
                "font_size":"Medium",
                "cw_behavior":"Hide",
                "nsfw_behavior":"Hide",
                "display_mode":"StarryEyes"
            }"#,
        )
        .unwrap();

        assert_eq!(settings.theme, Theme::Mocha);
        assert!(!settings.visibility_background_enabled);
    }
}
