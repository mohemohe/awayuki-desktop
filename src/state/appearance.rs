use gpui::Global;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AvatarShape {
    Square,
    Circle,
    Rounded,
}

impl AvatarShape {
    pub const ALL: [AvatarShape; 3] = [
        AvatarShape::Square,
        AvatarShape::Circle,
        AvatarShape::Rounded,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            AvatarShape::Square => "Square",
            AvatarShape::Circle => "Circle",
            AvatarShape::Rounded => "Rounded",
        }
    }

    /// Returns the border-radius in px for a given avatar size
    pub fn radius(&self, size: f32) -> f32 {
        match self {
            AvatarShape::Square => 0.0,
            AvatarShape::Circle => size / 2.0,
            AvatarShape::Rounded => (size * 0.15).max(4.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontSize {
    Small,
    Medium,
    Large,
}

impl FontSize {
    pub const ALL: [FontSize; 3] = [FontSize::Small, FontSize::Medium, FontSize::Large];

    pub fn label(&self) -> &'static str {
        match self {
            FontSize::Small => "Small",
            FontSize::Medium => "Medium",
            FontSize::Large => "Large",
        }
    }

    /// Font size in px for main content (replaces text_sm)
    pub fn content_px(&self) -> f32 {
        match self {
            FontSize::Small => 12.0,
            FontSize::Medium => 14.0,
            FontSize::Large => 16.0,
        }
    }

    /// Font size in px for secondary text (replaces text_xs)
    pub fn secondary_px(&self) -> f32 {
        match self {
            FontSize::Small => 10.0,
            FontSize::Medium => 12.0,
            FontSize::Large => 14.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CwBehavior {
    Hide,
    AlwaysExpand,
}

impl CwBehavior {
    pub const ALL: [CwBehavior; 2] = [CwBehavior::Hide, CwBehavior::AlwaysExpand];

    pub fn label(&self) -> &'static str {
        match self {
            CwBehavior::Hide => "Hide",
            CwBehavior::AlwaysExpand => "Always expand",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NsfwBehavior {
    Hide,
    AlwaysShow,
}

impl NsfwBehavior {
    pub const ALL: [NsfwBehavior; 2] = [NsfwBehavior::Hide, NsfwBehavior::AlwaysShow];

    pub fn label(&self) -> &'static str {
        match self {
            NsfwBehavior::Hide => "Hide",
            NsfwBehavior::AlwaysShow => "Always show",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub avatar_shape: AvatarShape,
    pub font_size: FontSize,
    pub cw_behavior: CwBehavior,
    pub nsfw_behavior: NsfwBehavior,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            avatar_shape: AvatarShape::Circle,
            font_size: FontSize::Medium,
            cw_behavior: CwBehavior::Hide,
            nsfw_behavior: NsfwBehavior::Hide,
        }
    }
}

impl Global for AppearanceSettings {}
