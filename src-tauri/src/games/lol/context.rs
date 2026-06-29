//! Build clip game-context (champion / map / mode / K-D-A / result) from the
//! League live snapshot, reusing the shared clip-context columns:
//! champion → `agent`, mapName → `map`, gameMode → `mode`, scores → K/D/A.

use crate::games::lol::events::MeId;
use crate::games::lol::live_client::AllGameData;
use crate::library::db::NewClip;

/// Our live context for tagging clips: champion, map, mode, and current K/D/A.
#[derive(Debug, Clone, Default)]
pub struct LolContext {
    /// Our identity across every name form the feed might use (matches event
    /// `KillerName`/`VictimName`/`Assisters`).
    pub me: MeId,
    /// Our team (`ORDER` / `CHAOS`) for team-wide events.
    pub team: String,
    pub champion: String,
    pub map: String,
    pub mode: String,
    pub kills: i64,
    pub deaths: i64,
    pub assists: i64,
}

impl LolContext {
    /// Resolve us from the live snapshot (our champion + live scoreboard).
    pub fn from_snapshot(data: &AllGameData) -> LolContext {
        // The primary name (used only to locate our scoreboard row).
        let me_name = data.active_player.name().to_string();
        // Gather every name form for ourselves — from `activePlayer` and, once
        // found, our `allPlayers` row — so event matching is robust to whichever
        // form the feed uses (summoner name / Riot ID game name / full Riot ID).
        let mut name_forms = data.active_player.name_forms();
        let our_entry = data.all_players.iter().find(|p| p.is(&me_name));
        if let Some(p) = our_entry {
            name_forms.extend(p.name_forms());
        }

        let mut ctx = LolContext {
            me: MeId::from_names(name_forms),
            team: data.active_player.team.clone(),
            map: friendly_map(&data.game_data.map_name),
            mode: friendly_mode(&data.game_data.game_mode),
            ..Default::default()
        };
        if let Some(p) = our_entry {
            ctx.champion = p.champion_name.clone();
            ctx.kills = p.scores.kills;
            ctx.deaths = p.scores.deaths;
            ctx.assists = p.scores.assists;
            if ctx.team.is_empty() {
                ctx.team = p.team.clone();
            }
        }
        ctx
    }

    /// A context-only [`NewClip`] (champion/map/mode/K-D-A). `won` is set by the
    /// caller at match end. `headshot_pct` is irrelevant for League (stays None).
    pub fn clip_context(&self, won: Option<bool>) -> NewClip {
        NewClip {
            agent: (!self.champion.is_empty()).then(|| self.champion.clone()),
            map: (!self.map.is_empty()).then(|| self.map.clone()),
            mode: (!self.mode.is_empty()).then(|| self.mode.clone()),
            won,
            kills: Some(self.kills),
            deaths: Some(self.deaths),
            assists: Some(self.assists),
            game: Some("lol".to_string()),
            ..Default::default()
        }
    }

    /// Title suffix for a clip (the champion name), empty when unknown.
    pub fn title_suffix(&self) -> String {
        self.champion.clone()
    }
}

/// Display name for a League game mode code (`CLASSIC` → "Summoner's Rift", etc.).
fn friendly_mode(code: &str) -> String {
    match code.to_ascii_uppercase().as_str() {
        "CLASSIC" => "Summoner's Rift",
        "ARAM" => "ARAM",
        "URF" | "ARURF" => "URF",
        "CHERRY" => "Arena",
        "NEXUSBLITZ" => "Nexus Blitz",
        "TUTORIAL" => "Tutorial",
        "PRACTICETOOL" => "Practice Tool",
        "" => "",
        other => other,
    }
    .to_string()
}

/// Normalize the map name (the feed already gives a readable name like "Summoner's
/// Rift"; pass through, trimming).
fn friendly_map(name: &str) -> String {
    name.trim().to_string()
}
