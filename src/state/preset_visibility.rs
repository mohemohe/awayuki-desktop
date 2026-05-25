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

    pub fn as_request_visibility(&self) -> &'static str {
        match self {
            VisibilityLevel::Public => "public",
            VisibilityLevel::Unlisted => "unlisted",
            VisibilityLevel::Private => "private",
            VisibilityLevel::Direct => "direct",
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
    /// Returns the first matching preset visibility. Keyword matching is
    /// case-insensitive. Empty keywords are ignored.
    pub fn match_visibility(&self, text: &str) -> Option<VisibilityLevel> {
        let lower = text.to_lowercase();
        for entry in &self.entries {
            let keyword = entry.keyword.trim();
            if keyword.is_empty() {
                continue;
            }
            if lower.contains(&keyword.to_lowercase()) {
                return Some(entry.visibility);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{PresetVisibilityEntry, PresetVisibilitySettings, VisibilityLevel};

    #[test]
    fn match_visibility_uses_first_matching_entry() {
        let settings = PresetVisibilitySettings {
            entries: vec![
                PresetVisibilityEntry {
                    keyword: "alpha".to_string(),
                    visibility: VisibilityLevel::Unlisted,
                },
                PresetVisibilityEntry {
                    keyword: "alpha".to_string(),
                    visibility: VisibilityLevel::Direct,
                },
            ],
        };

        assert_eq!(
            settings.match_visibility("contains alpha"),
            Some(VisibilityLevel::Unlisted)
        );
    }

    #[test]
    fn match_visibility_ignores_empty_entries_and_matches_case_insensitively() {
        let settings = PresetVisibilitySettings {
            entries: vec![
                PresetVisibilityEntry {
                    keyword: " ".to_string(),
                    visibility: VisibilityLevel::Direct,
                },
                PresetVisibilityEntry {
                    keyword: "Moon".to_string(),
                    visibility: VisibilityLevel::Private,
                },
            ],
        };

        assert_eq!(
            settings.match_visibility("moonlight"),
            Some(VisibilityLevel::Private)
        );
    }
}
