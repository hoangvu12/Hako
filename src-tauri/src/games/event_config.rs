//! `event_config!` — generates a game's per-event auto-clip config triplet.
//!
//! Every smart integration needs the same three types: an `*EventToggles`
//! (one bool per event kind the game emits), an `*EventTiming` (the
//! before/after clip window for one event), and an `*EventTimings` (the window
//! per event kind). Each was ~180 lines of identical boilerplate — the struct,
//! its `Default`, and the `enabled` / `for_kind` / `max_after` methods — where
//! the *only* thing that varied game to game was a table of
//! `(field, EventKind, default-on, window)` rows.
//!
//! [`event_config!`] takes exactly that table and expands to the same code, so
//! adding an event is one row instead of ~7 parallel edits across the file.
//!
//! Crucially, the field identifiers are written out verbatim at each call site
//! — they are the on-disk `settings.json` keys and must never change — so this
//! is a pure refactor of *how* the config types are written, not of their
//! serialized form. The `EventKind` each field maps to sits right beside it, so
//! the (deliberately non-uniform) field↔kind mapping stays explicit rather than
//! being derived.

/// Generate a game's `$Toggles` / `$Timing` / `$Timings` config types.
///
/// ```ignore
/// event_config! {
///     toggles: Cs2EventToggles,
///     timing: Cs2EventTiming,
///     timings: Cs2EventTimings,
///     // Window used when a partial `$Timing` omits a field, and the fallback
///     // `after` for `max_after` when no event is enabled.
///     default_window: (6, 5),
///     merge_fallback_after: 4,
///     events: {
///         kill        => EventKind::Kill,       on: false, window: (6, 5),
///         headshot    => EventKind::Headshot,   on: true,  window: (6, 5),
///         // …
///     },
/// }
/// ```
macro_rules! event_config {
    (
        toggles: $Toggles:ident,
        timing: $Timing:ident,
        timings: $Timings:ident,
        default_window: ($def_before:expr, $def_after:expr),
        merge_fallback_after: $merge_fallback:expr,
        events: {
            $(
                $field:ident => $kind:path, on: $on:expr, window: ($before:expr, $after:expr)
            ),+ $(,)?
        } $(,)?
    ) => {
        /// Per-event auto-clip toggles (one bool per event kind this game emits).
        /// Additive: `#[serde(default)]` fills any missing field from [`Default`].
        #[derive(Debug, Clone, Copy, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(default)]
        pub struct $Toggles {
            $(pub $field: bool,)+
        }

        impl Default for $Toggles {
            fn default() -> Self {
                $Toggles { $($field: $on,)+ }
            }
        }

        impl $Toggles {
            /// Whether clips are enabled for `kind` (kinds this game never emits
            /// are always `false`).
            pub fn enabled(&self, kind: $crate::games::event::EventKind) -> bool {
                match kind {
                    $($kind => self.$field,)+
                    _ => false,
                }
            }
        }

        /// One event's clip window: seconds to keep before / after the moment.
        #[derive(Debug, Clone, Copy, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(default)]
        pub struct $Timing {
            pub before: u32,
            pub after: u32,
        }

        impl Default for $Timing {
            fn default() -> Self {
                $Timing { before: $def_before, after: $def_after }
            }
        }

        /// Per-event clip windows for this game.
        #[derive(Debug, Clone, Copy, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(default)]
        pub struct $Timings {
            $(pub $field: $Timing,)+
        }

        impl Default for $Timings {
            fn default() -> Self {
                $Timings {
                    $($field: $Timing { before: $before, after: $after },)+
                }
            }
        }

        impl $Timings {
            /// The clip window for `kind` (a default window for kinds this game
            /// never emits).
            pub fn for_kind(&self, kind: $crate::games::event::EventKind) -> $Timing {
                match kind {
                    $($kind => self.$field,)+
                    _ => $Timing::default(),
                }
            }

            /// Widest `after`-pad across all *enabled* kinds (sizes the merge
            /// tolerance); falls back to a fixed value when nothing is enabled.
            pub fn max_after(&self, toggles: &$Toggles) -> u32 {
                let mut max: Option<u32> = None;
                $(
                    if toggles.$field {
                        let after = self.$field.after;
                        max = Some(max.map_or(after, |m| m.max(after)));
                    }
                )+
                max.unwrap_or($merge_fallback)
            }
        }
    };
}

pub(crate) use event_config;
