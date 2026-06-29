//! The multi-game supervisor.
//!
//! Replaces the single Valorant orchestrator spawn. On startup it spawns one
//! background task per registered [`GameIntegration`] (each owns its own polling
//! loop via `run`). They share the single global capture through the
//! [`crate::games::CaptureOwner`] arbiter, so only the game actually in a match
//! drives recording; a user's manual capture is never auto-stopped.

use tauri::AppHandle;

use crate::games::recording::GameCtx;
use crate::games::registry;

/// Spawn every game integration's live loop on the Tauri async runtime.
/// Idempotent per app (call once from `main`'s setup).
pub fn spawn(app: AppHandle) {
    for game in registry() {
        let name = game.id().display_name();
        let ctx = GameCtx::new(app.clone(), game.clone());
        tauri::async_runtime::spawn(game.run(ctx));
        tracing::info!("games: spawned {name} integration");
    }
}
