//! Post-match summary (K/D/A, headshot %, agent, map, win/loss, match title).
//!
//! Port of Medal's `ValorantPostMatchHandler.{UpdateGameStateWithMatchResults,
//! CalculateHeadshotPercentage,BuildMatchTitle}` and the
//! `PostMatchOverlayNotification` it pushes to its desktop app. Hako surfaces
//! this as a `match-summary` event (for the `/valorant` panel) and uses the
//! resolved agent name to label auto-clips.
//!
//! Pure + unit-tested. The agent **display name** is the one piece left to the
//! caller: it's an async web lookup (`remote_api::fetch_agent_name`), so
//! [`build_summary`] fills `agent_id` and a provisional title, and the cut
//! pipeline finalizes `agent` + `title` once the name resolves.

use serde::Serialize;

use crate::valorant::model::{game_mode_name, queue_id_name, MatchDetails};

/// Everything we show after a match ends. Serialized to the webview as the
/// `match-summary` event payload.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MatchSummary {
    pub kills: i32,
    pub deaths: i32,
    pub assists: i32,
    /// Headshot % over all the player's recorded damage (0–100).
    pub headshot_pct: f64,
    /// Raw agent UUID (`characterId`); resolved to `agent` via valorant-api.com.
    pub agent_id: String,
    /// Agent display name (filled by the cut pipeline; empty until resolved).
    pub agent: String,
    /// Map asset path — the UI prettifies it, same as the live presence map.
    pub map: String,
    /// Game-mode display name (e.g. "Standard", "Spike Rush").
    pub mode: String,
    pub won: bool,
    /// Match length in ms (`gameLengthMillis`).
    pub duration_ms: i64,
    /// Built title, e.g. "🟩 Victory - Jett [21/14/5]".
    pub title: String,
}

impl MatchSummary {
    /// `K/D/A` string.
    pub fn kda(&self) -> String {
        format!("{}/{}/{}", self.kills, self.deaths, self.assists)
    }

    /// The Valorant game-context fields for a clip cut from this match — agent,
    /// map, mode, result, K/D/A, headshot %. Returned as a context-only
    /// [`NewClip`] (path/title/media stay `Default`) for struct-update merge into
    /// the finalized clip. Empty agent/map/mode collapse to `None`.
    pub fn clip_context(&self) -> crate::library::db::NewClip {
        let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
        crate::library::db::NewClip {
            agent: some(&self.agent),
            agent_id: some(&self.agent_id),
            map: some(&self.map),
            mode: some(&self.mode),
            won: Some(self.won),
            kills: Some(self.kills as i64),
            deaths: Some(self.deaths as i64),
            assists: Some(self.assists as i64),
            headshot_pct: Some(self.headshot_pct),
            ..Default::default()
        }
    }

    /// Medal's `BuildMatchTitle`: outcome + agent + KDA. Uses the resolved
    /// `agent`, or "Unknown" when it hasn't been looked up.
    pub fn build_title(&self) -> String {
        let outcome = if self.won { "🟩 Victory" } else { "🟥 Defeat" };
        let agent = if self.agent.is_empty() {
            "Unknown"
        } else {
            self.agent.as_str()
        };
        format!("{outcome} - {agent} [{}]", self.kda())
    }
}

/// Build the post-match summary for `puuid` from match-details. The agent
/// display name and final title are filled in by the caller after the async
/// agent lookup (here `agent` is empty and `title` uses "Unknown").
pub fn build_summary(details: &MatchDetails, puuid: &str) -> MatchSummary {
    // Prefer the live-queue display name so the bomb-based queues read as their
    // actual mode (Competitive / Unrated / Swiftplay / Premier) instead of the
    // generic "Standard" gameMode, and so auto-clips match the live/manual clip
    // labels (the orchestrator tags those via `queue_id_name` too). Fall back to
    // the post-match gameMode asset name — which covers custom games, where the
    // queue id is absent — then to the raw queue id as a last resort.
    let mode = {
        let queue = &details.match_info.queue_id;
        let queue_name = queue_id_name(queue);
        if !queue_name.is_empty() {
            queue_name.to_string()
        } else {
            let asset_name = game_mode_name(&details.match_info.game_mode);
            if asset_name.is_empty() {
                queue.clone()
            } else {
                asset_name.to_string()
            }
        }
    };

    let mut s = MatchSummary {
        map: details.match_info.map_id.clone(),
        duration_ms: details.match_info.game_length_millis,
        mode,
        headshot_pct: headshot_pct(details, puuid),
        ..Default::default()
    };

    if let Some(p) = details.players.iter().find(|p| p.puuid == puuid) {
        s.kills = p.stats.kills;
        s.deaths = p.stats.deaths;
        s.assists = p.stats.assists;
        s.agent_id = p.character_id.clone();
        s.won = details
            .teams
            .iter()
            .find(|t| t.team_id == p.team_id)
            .map(|t| t.won)
            .unwrap_or(false);
    }

    s.title = s.build_title(); // provisional (agent not yet resolved)
    s
}

/// Headshot % over the player's recorded damage (Medal's
/// `CalculateHeadshotPercentage`). 0 when no damage is recorded.
fn headshot_pct(details: &MatchDetails, puuid: &str) -> f64 {
    let mut head = 0i64;
    let mut total = 0i64;
    for round in &details.round_results {
        for ps in round.player_stats.iter().filter(|ps| ps.puuid == puuid) {
            for d in &ps.damage {
                head += d.headshots as i64;
                total += (d.legshots + d.bodyshots + d.headshots) as i64;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        head as f64 / total as f64 * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::valorant::model::{
        DamageEvent, MatchInfo, Player, PlayerRoundStats, PlayerStats, RoundResult, Team,
    };

    fn player(puuid: &str, team: &str, agent: &str, k: i32, d: i32, a: i32) -> Player {
        Player {
            puuid: puuid.into(),
            game_name: String::new(),
            tag_line: String::new(),
            team_id: team.into(),
            character_id: agent.into(),
            stats: PlayerStats {
                kills: k,
                deaths: d,
                assists: a,
            },
        }
    }

    fn details() -> MatchDetails {
        MatchDetails {
            match_info: MatchInfo {
                map_id: "/Game/Maps/Ascent/Ascent".into(),
                game_length_millis: 1_800_000,
                game_mode: "/Game/GameModes/Bomb/BombGameMode.BombGameMode_C".into(),
                ..Default::default()
            },
            players: vec![
                player("me", "Blue", "agent-uuid", 21, 14, 5),
                player("them", "Red", "other", 10, 18, 3),
            ],
            teams: vec![
                Team {
                    team_id: "Blue".into(),
                    won: true,
                },
                Team {
                    team_id: "Red".into(),
                    won: false,
                },
            ],
            round_results: vec![RoundResult {
                round_num: 0,
                player_stats: vec![PlayerRoundStats {
                    puuid: "me".into(),
                    kills: vec![],
                    // 3 headshots of 4 shots = 75%.
                    damage: vec![DamageEvent {
                        legshots: 0,
                        bodyshots: 1,
                        headshots: 3,
                    }],
                }],
                ..Default::default()
            }],
        }
    }

    #[test]
    fn summarizes_kda_agent_and_result() {
        let s = build_summary(&details(), "me");
        assert_eq!((s.kills, s.deaths, s.assists), (21, 14, 5));
        assert_eq!(s.kda(), "21/14/5");
        assert_eq!(s.agent_id, "agent-uuid");
        assert!(s.won);
        assert_eq!(s.mode, "Standard");
        assert_eq!(s.duration_ms, 1_800_000);
    }

    #[test]
    fn computes_headshot_percentage() {
        let s = build_summary(&details(), "me");
        assert!((s.headshot_pct - 75.0).abs() < 1e-9);
    }

    #[test]
    fn headshot_pct_zero_without_damage() {
        let mut d = details();
        d.round_results[0].player_stats[0].damage.clear();
        let s = build_summary(&d, "me");
        assert_eq!(s.headshot_pct, 0.0);
    }

    #[test]
    fn title_uses_outcome_agent_and_kda() {
        let mut s = build_summary(&details(), "me");
        // Provisional title before the agent name resolves.
        assert_eq!(s.title, "🟩 Victory - Unknown [21/14/5]");
        // After the caller fills the resolved agent name.
        s.agent = "Jett".into();
        assert_eq!(s.build_title(), "🟩 Victory - Jett [21/14/5]");
    }

    #[test]
    fn defeat_title_for_losing_team() {
        let s = build_summary(&details(), "them");
        assert!(!s.won);
        assert_eq!(s.build_title(), "🟥 Defeat - Unknown [10/18/3]");
    }

    #[test]
    fn bomb_queues_label_by_queue_not_generic_standard() {
        // Competitive/Unrated/etc. all carry the Bomb gameMode ("Standard"); the
        // queue id is what tells them apart, so we prefer it — auto-clips then read
        // "Competitive" instead of the generic "Standard" (and match manual clips).
        let mut d = details(); // gameMode is Bomb
        d.match_info.queue_id = "competitive".into();
        assert_eq!(build_summary(&d, "me").mode, "Competitive");

        d.match_info.queue_id = "unrated".into();
        assert_eq!(build_summary(&d, "me").mode, "Unrated");

        // No queue id (custom game) → fall back to the gameMode asset name.
        d.match_info.queue_id = "".into();
        assert_eq!(build_summary(&d, "me").mode, "Standard");
    }

    #[test]
    fn mode_falls_back_to_queue_name_when_asset_unmapped() {
        let mut d = details();
        d.match_info.game_mode = "/Game/GameModes/Unknown/Whatever_C".into();
        d.match_info.queue_id = "competitive".into();
        let s = build_summary(&d, "me");
        // Prettified via the queue table, not the raw lowercase id.
        assert_eq!(s.mode, "Competitive");
    }

    #[test]
    fn team_deathmatch_resolves_via_queue_when_asset_unmapped() {
        // A TDM match whose gameMode asset isn't in the table still labels as
        // "Team Deathmatch" through the queue-id fallback (regression: it used to
        // leak the raw "hurm" id).
        let mut d = details();
        d.match_info.game_mode = "/Game/GameModes/Some/Future_C".into();
        d.match_info.queue_id = "hurm".into();
        let s = build_summary(&d, "me");
        assert_eq!(s.mode, "Team Deathmatch");
    }

    #[test]
    fn mode_falls_back_to_raw_id_when_fully_unknown() {
        let mut d = details();
        d.match_info.game_mode = "/Game/GameModes/Unknown/Whatever_C".into();
        d.match_info.queue_id = "somecustomqueue".into();
        let s = build_summary(&d, "me");
        assert_eq!(s.mode, "somecustomqueue");
    }
}
