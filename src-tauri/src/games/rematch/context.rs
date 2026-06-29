//! Clip game-context for Rematch, built from the log: the player's name, the
//! game mode, and the loaded stadium. Reuses the shared clip-context columns —
//! stadium → `map`, mode → `mode` — so a Rematch clip tags like every other game.

use crate::library::db::NewClip;

/// What we know about the current Rematch match, for tagging its clips.
#[derive(Debug, Clone, Default)]
pub struct RematchContext {
    /// Local player display name (from `localPlayerNickname` / `steamNickname`).
    pub player: String,
    /// Game mode ("Ranked" / "Quick Match" / "Custom"), when seen in the menu.
    pub mode: String,
    /// Loaded stadium ("Coliseum", "Wind", "Super Goal", …).
    pub map: String,
}

impl RematchContext {
    /// A context-only [`NewClip`] (stadium / mode). Rematch has no agent/champion
    /// and no reliably-derivable win/loss, so those stay unset.
    pub fn clip_context(&self) -> NewClip {
        NewClip {
            map: (!self.map.is_empty()).then(|| self.map.clone()),
            mode: (!self.mode.is_empty()).then(|| self.mode.clone()),
            game: Some("rematch".to_string()),
            ..Default::default()
        }
    }

    /// Title suffix for a clip — the stadium, falling back to the mode, else empty
    /// (the cut then titles by duration instead).
    pub fn title_suffix(&self) -> String {
        if !self.map.is_empty() {
            self.map.clone()
        } else {
            self.mode.clone()
        }
    }
}
