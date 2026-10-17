//! Screen renderers for `story-tui`.

pub mod greeting;
pub mod home;
pub mod import;
pub mod keymap;
pub mod save_load;
pub mod settings;
pub mod story;

/// Keep provider-supplied model identifiers out of player-facing screens.
/// The stable fingerprint still lets a player distinguish entries and match
/// the same local model across the picker, settings, and story history.
pub(crate) fn model_display_label(model: &str) -> String {
    if model.trim().is_empty() {
        return "(미지정)".to_string();
    }
    let hash = model.as_bytes().iter().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    });
    format!("로컬 모델 #{hash:08X}")
}

pub use save_load::{
    MIN_HEIGHT as SAVE_LOAD_MIN_HEIGHT, MIN_WIDTH as SAVE_LOAD_MIN_WIDTH, render_save_load,
};
pub use settings::{
    MIN_HEIGHT as SETTINGS_MIN_HEIGHT, MIN_WIDTH as SETTINGS_MIN_WIDTH, SettingsView,
    render_settings,
};

#[cfg(test)]
mod tests {
    use super::model_display_label;

    #[test]
    fn model_labels_are_stable_and_do_not_expose_provider_ids() {
        let raw = "provider/private-model-name";
        let first = model_display_label(raw);
        assert_eq!(first, model_display_label(raw));
        assert!(first.starts_with("로컬 모델 #"));
        assert!(!first.contains(raw));
    }
}
