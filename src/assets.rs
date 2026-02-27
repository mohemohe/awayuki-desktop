use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
struct CustomAssets;

/// Combined asset source: project-local assets with gpui-component-assets fallback.
pub struct CombinedAssets;

impl CombinedAssets {
    pub fn new() -> Self {
        Self
    }
}

impl AssetSource for CombinedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        // Try custom assets first
        if let Some(file) = CustomAssets::get(path) {
            return Ok(Some(file.data));
        }

        // Fallback to gpui-component-assets
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut results: Vec<SharedString> = CustomAssets::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| p.into())
            .collect();

        if let Ok(fallback) = gpui_component_assets::Assets.list(path) {
            for item in fallback {
                if !results.contains(&item) {
                    results.push(item);
                }
            }
        }

        Ok(results)
    }
}
