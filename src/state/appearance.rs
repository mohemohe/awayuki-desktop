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
    pub display_mode: DisplayMode,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            avatar_shape: AvatarShape::Circle,
            font_size: FontSize::Medium,
            cw_behavior: CwBehavior::Hide,
            nsfw_behavior: NsfwBehavior::Hide,
            display_mode: DisplayMode::StarryEyes,
        }
    }
}
