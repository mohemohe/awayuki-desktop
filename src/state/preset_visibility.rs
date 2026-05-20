use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisibilityLevel {
    Public,
    Unlisted,
    Private,
    Direct,
}

impl VisibilityLevel {
    pub const ALL: [VisibilityLevel; 4] = [
        VisibilityLevel::Public,
        VisibilityLevel::Unlisted,
        VisibilityLevel::Private,
        VisibilityLevel::Direct,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            VisibilityLevel::Public => "Public",
            VisibilityLevel::Unlisted => "Unlisted",
            VisibilityLevel::Private => "Private",
            VisibilityLevel::Direct => "Direct",
        }
    }

    /// Row index in the compose visibility select (Public, Unlisted, Private, Direct).
    pub fn select_row(&self) -> usize {
        match self {
            VisibilityLevel::Public => 0,
            VisibilityLevel::Unlisted => 1,
            VisibilityLevel::Private => 2,
            VisibilityLevel::Direct => 3,
        }
    }

    /// Higher value = stricter (less visible).
    pub fn strictness(&self) -> u8 {
        match self {
            VisibilityLevel::Public => 0,
            VisibilityLevel::Unlisted => 1,
            VisibilityLevel::Private => 2,
            VisibilityLevel::Direct => 3,
        }
    }
}

impl Default for VisibilityLevel {
    fn default() -> Self {
        VisibilityLevel::Public
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetVisibilityEntry {
    pub keyword: String,
    pub visibility: VisibilityLevel,
}

impl Default for PresetVisibilityEntry {
    fn default() -> Self {
        Self {
            keyword: String::new(),
            visibility: VisibilityLevel::Unlisted,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PresetVisibilitySettings {
    #[serde(default)]
    pub entries: Vec<PresetVisibilityEntry>,
}

impl PresetVisibilitySettings {
    /// Returns the strictest visibility among presets whose keyword appears in `text`.
    /// Keyword matching is case-insensitive. Empty keywords are ignored.
    pub fn match_visibility(&self, text: &str) -> Option<VisibilityLevel> {
        let lower = text.to_lowercase();
        let mut best: Option<VisibilityLevel> = None;
        for entry in &self.entries {
            let keyword = entry.keyword.trim();
            if keyword.is_empty() {
                continue;
            }
            if lower.contains(&keyword.to_lowercase()) {
                best = Some(match best {
                    Some(current) if current.strictness() >= entry.visibility.strictness() => {
                        current
                    }
                    _ => entry.visibility,
                });
            }
        }
        best
    }
}
