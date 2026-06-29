//! `Runtime.log` parsing for Rematch — the source of every Rematch event.
//!
//! Rematch has no local API and no goal feed; Medal and Overwolf both reverse-
//! engineer it (Medal server-side, Overwolf via a native memory-reading plugin).
//! But the game's Unreal log carries everything we need for *highlights* in plain
//! text, so — exactly like Valorant's `ShooterGame.log` tail — we follow it:
//!
//! - **Goal scored** → `… RuntimeMatchSound: GoalScored …` (the goal-sound cue;
//!   fires once per goal). This is our lone highlight event, matching Medal's
//!   "Goal Scored".
//! - **Match start** → `GameFlow.States.Match CountdownOver` (kickoff).
//! - **Match end** → `GameFlow.States.EndMatchWhistle` / `…MatchEnd…`.
//! - **Context** → `localPlayerNickname:`, the menu mode lines, and the loaded
//!   stadium map (purely for clip tagging).
//!
//! The goal cue is enough timing-wise: we tail on a ~1 s poll and clips pad ±8 s,
//! so memory-precise timing (why Overwolf reads memory for its overlay) buys us
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

/// The goal-sound cue Rematch logs once per goal scored in the match.
const GOAL_MARKER: &str = "RuntimeMatchSound: GoalScored";

/// Whether `line` is a goal-scored cue.
pub fn is_goal_scored(line: &str) -> bool {
    line.contains(GOAL_MARKER)
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
    line.contains("GameFlow.States.EndMatchWhistle")
        || line.contains("GameFlow.States.MatchEnd")
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
        let line = "[2026.06.28-23.44.23:049][483]LogBlueprintUserMessages: \
                    [BP_RuntimeMatchSound_C_2147440802] Ebb: RuntimeMatchSound: GoalScored \
                    -> Branch Failed: false true true";
        assert!(is_goal_scored(line));
        assert!(!is_goal_scored("LogBlueprintUserMessages: RuntimeMatchSound: WhistleBlown"));
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
        assert_eq!(parse_player_name("{ localPlayerNickname: , foo: bar }"), None);
        // steamNickname fallback.
        assert_eq!(
            parse_player_name("{ steamNickname: katou, isSteamEnabled: true }").as_deref(),
            Some("katou")
        );
        assert_eq!(parse_player_name("no nickname here"), None);
    }

    #[test]
    fn parses_game_mode() {
        assert_eq!(parse_game_mode("LogTemp: Display: StartCustomMatch begin"), Some("Custom"));
        assert_eq!(
            parse_game_mode("LogUIActionRouter: Display: [User 0] Focused desired target Btn_RankedMatch"),
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
