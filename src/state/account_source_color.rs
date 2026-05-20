use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AccountSourceColor {
    #[default]
    Transparent,
    Mauve,
    Red,
    Peach,
    Yellow,
    Green,
    Sapphire,
    Lavender,
}
