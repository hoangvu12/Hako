//! `Runtime.log` parsing for Rematch — the source of every Rematch event.
//!
//! Rematch has no local API and no goal feed; Medal and Overwolf both reverse-
//! engineer it (Medal server-side, Overwolf via a native memory-reading plugin
//! plus on-screen score-overlay vision models). But the game's Unreal log carries
//! everything we need for *highlights* in plain text, so — exactly like Valorant's
//! `ShooterGame.log` tail — we follow it:
//!
//! - **Goal scored** → `… GameFlow.States.Celebration.FreeRun PostGoal …` (the
//!   post-goal celebration transition; fires exactly once per goal), matching
//!   Medal's "Goal Scored". Note: the game *also* logs a
//!   `RuntimeMatchSound: GoalScored` audio cue, but it is unreliable — it
//!   fires for only a small fraction of goals (~1 in 15 observed), so we key on
//!   the celebration state instead, which is 1:1 with goals (as is the paired
//!   `RuntimeGoalReplay` demo playback).
//! - **Goal/assist by the local player** → the game's achievement layer pushes
//!   the local player's lifetime Steam stats and logs each increment:
//!   `LogSCAchievement: New value for Stat Goals is <n> for User <name>` (same
//!   for `Stat Assists`), observed ~1 s after the goal's `PostGoal` cue — for
//!   the local player's own goals/assists *only*. So a stat increment shortly
//!   after a goal cue attributes that goal ("scored by me" / "assisted by me");
//!   no increment means a teammate's or the enemy team's goal. This is
//!   attribution Overwolf's public Rematch GEP doesn't even expose (it only has
//!   team-level `team_goal`/`opponent_goal`).
//! - **Match start** → `GameFlow.States.Match CountdownOver` (kickoff).
//! - **Match end** → `GameFlow.States.EndMatchWhistle` / `…MatchEnd…`.
//! - **Context** → `localPlayerNickname:`, the menu mode lines, and the loaded
//!   stadium map (purely for clip tagging).
//!
//! `PostGoal` fires at the celebration, a few seconds *after* the ball crosses,
//! but that is fine timing-wise: we tail on a ~1 s poll and the goal clip pads
//! well back (`before` ≫ that lag), so the shot itself lands inside the window.
//! Memory-precise timing (why Overwolf reads memory for its overlay) buys us
//! nothing. The actual IO tail + read-time→event backdating is shared with
//! Valorant ([`crate::valorant::log_watch::LogTail`] / `line_event_ticks`); this
//! module is the pure, unit-tested parsing layer.

#![allow(dead_code)]

use std::path::PathBuf;

/// Resolve `Runtime.log` — `%LOCALAPPDATA%\Runtime\Saved\Logs\Runtime.log`. The
/// UE project is named "Runtime" (hence the process `RuntimeClient-*` and this
/// path), so the folder is `Runtime`, not `Rematch`.
pub fn log_path() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let p = PathBuf::from(&local)
        .join("Runtime")
        .join("Saved")
        .join("Logs")
        .join("Runtime.log");
    p.exists().then_some(p)
}

/// The post-goal celebration transition Rematch logs exactly once per goal. We
/// key on this rather than the `RuntimeMatchSound: GoalScored` audio cue, which
/// fires for only a fraction of goals (see the module docs).
const GOAL_MARKER: &str = "GameFlow.States.Celebration.FreeRun PostGoal";

/// Whether `line` is a goal-scored cue (the post-goal celebration transition).
pub fn is_goal_scored(line: &str) -> bool {
    line.contains(GOAL_MARKER)
}

/// Whether `line` is the local player's goal-count stat increment — logged by
/// the achievement layer ~1 s after the goal cue when *we* scored. Matching
/// only the `New value` line (each increment logs an `Old value`/`New value`
/// pair) keeps this 1:1 with our goals.
pub fn is_my_goal_stat(line: &str) -> bool {
    line.contains("LogSCAchievement") && line.contains("New value for Stat Goals is")
}

/// Whether `line` is the local player's assist-count stat increment — logged
/// ~1 s after a teammate's goal that we assisted (see [`is_my_goal_stat`]).
pub fn is_my_assist_stat(line: &str) -> bool {
    line.contains("LogSCAchievement") && line.contains("New value for Stat Assists is")
}

/// Whether `line` is the kickoff (match went live). The countdown finished and
/// the ball is in play — `GameFlow.States.Match CountdownOver`.
pub fn is_match_start(line: &str) -> bool {
    line.contains("GameFlow.States.Match CountdownOver")
}

/// Whether `line` is a match-ended transition (final whistle / MatchEnd state).
/// Several end-class lines fire in quick succession; the caller finalizes on the
/// first and ignores the rest (the active match is already taken).
pub fn is_match_end(line: &str) -> bool {
    line.contains("GameFlow.States.EndMatchWhistle") || line.contains("GameFlow.States.MatchEnd")
}

/// The local player's display name from a `… localPlayerNickname: <name> …` line
/// (or a `steamNickname: <name>`), trimmed. `None` for the empty placeholder the
/// game logs before sign-in (`localPlayerNickname: ,`).
pub fn parse_player_name(line: &str) -> Option<String> {
    for key in ["localPlayerNickname:", "steamNickname:"] {
        if let Some(rest) = line.split(key).nth(1) {
            // Value runs until the next ',' or '}' (the surrounding log is a
            // brace-delimited key/value dump).
            let val = rest
                .trim_start()
                .split(|c| c == ',' || c == '}')
                .next()
                .unwrap_or("")
                .trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// The game mode from a menu-interaction line, mirroring Overwolf's Rematch GEP
/// parser: the Quickmatch widget reports its mode (e.g. `Ranked3v3`), the custom-
/// match start and the ranked button are explicit. `None` for unrelated lines.
pub fn parse_game_mode(line: &str) -> Option<&'static str> {
    if line.contains("StartCustomMatch begin") {
        return Some("Custom");
    }
    if line.contains("Focused desired target Btn_RankedMatch") {
        return Some("Ranked");
    }
    if line.contains("WBP_Menu_Quickmatch_C") {
        return Some("Quick Match");
    }
    None
}

/// A friendly stadium name from a map path segment, e.g.
/// `StadiumColiseum_73x49_Night_Main` → "Coliseum", `Stadium_SuperGoal_73x49…`
/// → "Super Goal". Returns the loaded stadium for clip context, or `None` for a
/// non-stadium map (main menu).
pub fn parse_map(line: &str) -> Option<String> {
    // Only the in-match stadium maps (skip MainMenuMap and PIE editor maps).
    if !line.contains("LoadMap") && !line.contains("Welcomed by server") {
        return None;
    }
    // Scan the path segments for the map basename — the one that *starts with*
    // "Stadium" (so the `S2_T5_Stadium_01` folder, which only contains it, is
    // skipped in favor of the `Stadium_SuperGoal_…` file).
    for seg in line.split('/') {
        let token: String = seg
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if token.starts_with("Stadium") {
            return friendly_stadium(&token);
        }
    }
    None
}

/// Prettify a `Stadium…` map token into a display name. Strips the `Stadium`
/// prefix and the `_<W>x<H>_<TimeOfDay>_Main` size/variant suffix, then spaces
/// out CamelCase / underscores.
fn friendly_stadium(token: &str) -> Option<String> {
    let mut core = token.strip_prefix("Stadium").unwrap_or(token);
    core = core.trim_start_matches('_');
    // Drop the `_73x49_Night_Main`-style suffix: keep up to the first `_<digit>`.
    if let Some(pos) = core.find(|c: char| c == '_').and_then(|_| {
        core.char_indices()
            .find(|(i, c)| *c == '_' && core[i + 1..].starts_with(|d: char| d.is_ascii_digit()))
            .map(|(i, _)| i)
    }) {
        core = &core[..pos];
    }
    let core = core.trim_matches('_');
    if core.is_empty() {
        return None;
    }
    // Space out underscores and CamelCase boundaries.
    let mut out = String::new();
    let mut prev_lower = false;
    for ch in core.chars() {
        if ch == '_' {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            prev_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && prev_lower {
            out.push(' ');
        }
        out.push(ch);
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_goal_cue() {
        let line = "[2026.06.30-20.20.37:501][211]LogSCGameFlow: Display: GameFlow : \
                    GameFlow.States.Match, GameFlow.States.Celebration.FreeRun PostGoal";
        assert!(is_goal_scored(line));
        // The unreliable audio cue is intentionally NOT treated as a goal — it
        // fires for only a fraction of goals, so we key on the celebration state.
        assert!(!is_goal_scored(
            "[2026.06.28-23.44.23:049][483]LogBlueprintUserMessages: \
             [BP_RuntimeMatchSound_C_2147440802] Ebb: RuntimeMatchSound: GoalScored \
             -> Branch Failed: false true true"
        ));
        // A pre-goal / other celebration state must not match.
        assert!(!is_goal_scored(
            "GameFlow.States.Celebration.FreeRun PreKickoff"
        ));
    }

    #[test]
    fn detects_my_goal_and_assist_stats() {
        let goal = "[2026.07.19-12.01.56:347][934]LogSCAchievement: New value for Stat Goals \
                    is 497 for User katou [0x11000013E33CD65]";
        assert!(is_my_goal_stat(goal));
        assert!(!is_my_assist_stat(goal));
        let assist = "[2026.07.19-12.03.48:280][611]LogSCAchievement: New value for Stat Assists \
                      is 322 for User katou [0x11000013E33CD65]";
        assert!(is_my_assist_stat(assist));
        assert!(!is_my_goal_stat(assist));
        // The paired `Old value` line must NOT count — only the increment.
        assert!(!is_my_goal_stat(
            "[2026.07.19-12.01.56:347][934]LogSCAchievement: Old value for Stat Goals \
             is 496 for User katou [0x11000013E33CD65]"
        ));
        // Other stats (Saves, MatchesWon, …) are not goal/assist cues.
        assert!(!is_my_goal_stat(
            "LogSCAchievement: New value for Stat Saves is 443 for User katou"
        ));
        assert!(!is_my_assist_stat(
            "LogSCAchievement: New value for Stat MatchesWon is 405 for User katou"
        ));
    }

    #[test]
    fn detects_match_start_and_end() {
        assert!(is_match_start(
            "LogSCGameFlow: Display: GameFlow : GameFlow.States.Match.Countdown, \
             GameFlow.States.Match CountdownOver"
        ));
        assert!(!is_match_start("GameFlow.States.Menu Main Home"));
        assert!(is_match_end(
            "LogSCGameFlow: Display: GameFlow : GameFlow.States.EndMatchWhistle, \
             GameFlow.States.MatchEnd Client"
        ));
        assert!(is_match_end(
            "GameFlow : GameFlow.States.Match.Synchro, GameFlow.States.EndMatchWhistle MatchEnded"
        ));
        assert!(!is_match_end("GameFlow.States.Match CountdownOver"));
    }

    #[test]
    fn parses_player_name() {
        let line = "LogSlcpSos: Display: First party / EOS auth success { system: SOSRedPointAuth, \
                    localPlayerNickname: katou, easAuthenticationMethod: DevTool, steamId: 7656119 }";
        assert_eq!(parse_player_name(line).as_deref(), Some("katou"));
        // The pre-sign-in placeholder is empty → None.
        assert_eq!(
            parse_player_name("{ localPlayerNickname: , foo: bar }"),
            None
        );
        // steamNickname fallback.
        assert_eq!(
            parse_player_name("{ steamNickname: katou, isSteamEnabled: true }").as_deref(),
            Some("katou")
        );
        assert_eq!(parse_player_name("no nickname here"), None);
    }

    #[test]
    fn parses_game_mode() {
        assert_eq!(
            parse_game_mode("LogTemp: Display: StartCustomMatch begin"),
            Some("Custom")
        );
        assert_eq!(
            parse_game_mode(
                "LogUIActionRouter: Display: [User 0] Focused desired target Btn_RankedMatch"
            ),
            Some("Ranked")
        );
        assert_eq!(
            parse_game_mode("LogBlueprintUserMessages: [WBP_Menu_Quickmatch_C_2147] Ranked3v3"),
            Some("Quick Match")
        );
        assert_eq!(parse_game_mode("unrelated"), None);
    }

    #[test]
    fn parses_friendly_stadium() {
        let coliseum = "LogLoad: LoadMap: 54.169.122.252/Game/Maps/MainMaps/Coliseum/\
                        StadiumColiseum_73x49_Night_Main?EncryptionToken=abc";
        assert_eq!(parse_map(coliseum).as_deref(), Some("Coliseum"));
        let wind = "LogNet: Welcomed by server (Level: /Game/Maps/MainMaps/Wind/\
                    StadiumWind_73x49_Day_Main, Game: ...)";
        assert_eq!(parse_map(wind).as_deref(), Some("Wind"));
        let supergoal = "LogLoad: LoadMap(/Game/Maps/MainMaps/S2_T5_Stadium_01/\
                         Stadium_SuperGoal_73x49_Day_Main)";
        assert_eq!(parse_map(supergoal).as_deref(), Some("Super Goal"));
        assert_eq!(parse_map("LogLoad: LoadMap: /Game/Maps/MainMenuMap"), None);
    }
}
