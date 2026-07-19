//! Tests for the clip library.

use super::schema::relabel_legacy_standard;
use super::*;

#[test]
fn relabels_legacy_standard_once() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE clips (id INTEGER PRIMARY KEY, mode TEXT);
         INSERT INTO clips (mode) VALUES ('Standard'), ('Competitive'), ('Standard'), (NULL);",
    )
    .unwrap();

    // First pass (user_version 0): the two legacy "Standard" rows → "Unrated".
    relabel_legacy_standard(&conn).unwrap();
    let count = |m: &str| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM clips WHERE mode = ?1", [m], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert_eq!(count("Unrated"), 2);
    assert_eq!(count("Standard"), 0);
    assert_eq!(count("Competitive"), 1); // other modes untouched

    // A later custom-game "Standard" clip is preserved — the guard makes the
    // second pass a no-op (it has no queue id and is legitimately "Standard").
    conn.execute("INSERT INTO clips (mode) VALUES ('Standard')", [])
        .unwrap();
    relabel_legacy_standard(&conn).unwrap();
    assert_eq!(count("Standard"), 1);
    assert_eq!(count("Unrated"), 2);
}

fn sample(path: &str, title: &str, event: Option<&str>) -> NewClip {
    NewClip {
        path: path.into(),
        title: title.into(),
        event: event.map(|s| s.into()),
        events: event.into_iter().map(|s| s.to_string()).collect(),
        duration_secs: 12.0,
        width: 2560,
        height: 1440,
        size_bytes: 1234,
        thumb_path: None,
        filmstrip_path: None,
        ..Default::default()
    }
}

#[test]
fn insert_list_rename_delete() {
    let lib = Library::open_in_memory().unwrap();
    let id1 = lib.insert(&sample("a.mp4", "First", Some("Ace"))).unwrap();
    let _id2 = lib.insert(&sample("b.mp4", "Second", None)).unwrap();
    assert_eq!(lib.count().unwrap(), 2);

    let all = lib.list().unwrap();
    assert_eq!(all.len(), 2);
    // Newest first; both inserted ~same ms, so just check membership.
    assert!(all
        .iter()
        .any(|c| c.title == "First" && c.event.as_deref() == Some("Ace")));

    lib.rename(id1, "Renamed").unwrap();
    assert_eq!(lib.get(id1).unwrap().unwrap().title, "Renamed");

    lib.delete(id1).unwrap();
    assert!(lib.get(id1).unwrap().is_none());
    assert_eq!(lib.count().unwrap(), 1);
}

#[test]
fn multi_event_round_trips_and_single_falls_back() {
    let lib = Library::open_in_memory().unwrap();
    // A merged window carrying several events.
    let mut multi = sample("m.mp4", "Spike Defused + Kill", Some("Spike Defused"));
    multi.events = vec!["Spike Defused".into(), "Kill".into()];
    let id_multi = lib.insert(&multi).unwrap();
    let rec = lib.get(id_multi).unwrap().unwrap();
    assert_eq!(rec.events, vec!["Spike Defused", "Kill"]);
    assert_eq!(rec.event.as_deref(), Some("Spike Defused"));

    // A single-event clip persisted with an empty events list still reports
    // its one event (the `event` fallback the UI relies on).
    let mut single = sample("s.mp4", "Ace", Some("Ace"));
    single.events = Vec::new();
    let id_single = lib.insert(&single).unwrap();
    assert_eq!(lib.get(id_single).unwrap().unwrap().events, vec!["Ace"]);
}

#[test]
fn game_context_round_trips_and_defaults_to_null() {
    let lib = Library::open_in_memory().unwrap();

    // A fully-enriched auto-clip.
    let mut enriched = sample("g.mp4", "Ace — Jett", Some("Ace"));
    enriched.agent = Some("Jett".into());
    enriched.agent_id = Some("add6443a-41bd-e414-f6ad-e58d267f4e95".into());
    enriched.map = Some("/Game/Maps/Ascent/Ascent".into());
    enriched.mode = Some("Competitive".into());
    enriched.won = Some(true);
    enriched.kills = Some(21);
    enriched.deaths = Some(14);
    enriched.assists = Some(5);
    enriched.headshot_pct = Some(31.5);
    let id = lib.insert(&enriched).unwrap();
    let rec = lib.get(id).unwrap().unwrap();
    assert_eq!(rec.agent.as_deref(), Some("Jett"));
    assert_eq!(rec.map.as_deref(), Some("/Game/Maps/Ascent/Ascent"));
    assert_eq!(rec.mode.as_deref(), Some("Competitive"));
    assert_eq!(rec.won, Some(true));
    assert_eq!(
        (rec.kills, rec.deaths, rec.assists),
        (Some(21), Some(14), Some(5))
    );
    assert_eq!(rec.headshot_pct, Some(31.5));

    // A bare clip (manual save with no match context) → all game fields null.
    let bare = sample("b.mp4", "Clip", None);
    let bid = lib.insert(&bare).unwrap();
    let brec = lib.get(bid).unwrap().unwrap();
    assert_eq!(brec.agent, None);
    assert_eq!(brec.map, None);
    assert_eq!(brec.won, None);
    assert_eq!(brec.kills, None);
}

#[test]
fn event_marks_round_trip_and_rebase() {
    let lib = Library::open_in_memory().unwrap();
    let mut c = sample("e.mp4", "Double Kill", Some("Double Kill"));
    c.event_marks = vec![
        EventMark {
            label: "Kill".into(),
            at: 3.0,
        },
        EventMark {
            label: "Kill".into(),
            at: 9.5,
        },
    ];
    let id = lib.insert(&c).unwrap();
    let rec = lib.get(id).unwrap().unwrap();
    assert_eq!(rec.event_marks.len(), 2);
    assert_eq!(rec.event_marks[1].at, 9.5);

    // Rebasing onto [2, 8) drops the 9.5s mark and shifts 3.0 → 1.0.
    let rebased = rebase_marks(&rec.event_marks, 2.0, 8.0);
    assert_eq!(rebased.len(), 1);
    assert_eq!(rebased[0].at, 1.0);

    // A clip with no marks round-trips to an empty list (NULL column).
    let bare = sample("b.mp4", "Clip", None);
    let bid = lib.insert(&bare).unwrap();
    assert!(lib.get(bid).unwrap().unwrap().event_marks.is_empty());
}

#[test]
fn rename_missing_errors() {
    let lib = Library::open_in_memory().unwrap();
    assert!(lib.rename(999, "x").is_err());
}

#[test]
fn cloud_upload_lifecycle_and_cascade() {
    let lib = Library::open_in_memory().unwrap();
    let id = lib.insert(&sample("c.mp4", "Clip", None)).unwrap();

    // Enqueue → uploading → progress → done.
    lib.cloud_enqueue(id, "r2-main", 1000).unwrap();
    let q = &lib.cloud_status(Some(id)).unwrap()[0];
    assert_eq!(q.status, cloud_status::QUEUED);
    assert_eq!((q.size_bytes, q.bytes_sent, q.uploaded_at), (1000, 0, None));

    lib.cloud_mark_uploading(id, "r2-main", "hako/2026/06/c.mp4")
        .unwrap();
    lib.cloud_set_progress(id, "r2-main", 512).unwrap();
    lib.cloud_mark_done(id, "r2-main", Some("https://signed/url"))
        .unwrap();
    let d = &lib.cloud_status(Some(id)).unwrap()[0];
    assert_eq!(d.status, cloud_status::DONE);
    assert_eq!(d.bytes_sent, d.size_bytes); // snapped to total on success
    assert!(d.uploaded_at.is_some()); // eviction gate set
    assert_eq!(d.remote_url.as_deref(), Some("https://signed/url"));
    assert_eq!(d.remote_path.as_deref(), Some("hako/2026/06/c.mp4"));

    // Re-enqueue resets progress/error/uploaded_at in place (a retry).
    lib.cloud_mark_failed(id, "r2-main", cloud_status::ERROR, "boom")
        .unwrap();
    lib.cloud_enqueue(id, "r2-main", 1000).unwrap();
    let r = &lib.cloud_status(Some(id)).unwrap()[0];
    assert_eq!(r.status, cloud_status::QUEUED);
    assert_eq!(r.error, None);
    assert_eq!(r.uploaded_at, None);
    assert_eq!(lib.cloud_status(Some(id)).unwrap().len(), 1); // overwrote, no dup

    // Deleting the clip cascades the cloud row away (PRAGMA foreign_keys=ON).
    lib.delete(id).unwrap();
    assert!(lib.cloud_status(Some(id)).unwrap().is_empty());
}

#[test]
fn retention_only_evicts_uploaded_clips() {
    let lib = Library::open_in_memory().unwrap();
    // Two clips, 1000 bytes each (see `sample`'s size_bytes = 1234 actually).
    let mut uploaded_clip = sample("done.mp4", "Uploaded", None);
    uploaded_clip.thumb_path = Some("done.jpg".into());
    uploaded_clip.filmstrip_path = Some("done_strip.jpg".into());
    let uploaded = lib.insert(&uploaded_clip).unwrap();
    let local_only = lib.insert(&sample("local.mp4", "Local", None)).unwrap();

    // Only the first is safely in the cloud → only it is an eviction candidate.
    lib.cloud_enqueue(uploaded, "r2-main", 1234).unwrap();
    lib.cloud_mark_done(uploaded, "r2-main", Some("https://signed/url"))
        .unwrap();

    let (bytes_before, count_before) = lib.local_footprint().unwrap();
    assert_eq!(count_before, 2);
    assert_eq!(bytes_before, 1234 * 2);

    let candidates = lib.evictable_clips().unwrap();
    assert_eq!(candidates.len(), 1, "only the uploaded clip is evictable");
    assert_eq!(candidates[0].id, uploaded);
    // The completed-upload provider is surfaced for the presign gate.
    assert_eq!(candidates[0].provider_ids, vec!["r2-main".to_string()]);

    // Evicting flips the flag and drops it from the footprint, but KEEPS the
    // thumbnail/filmstrip (only the video is deleted) so the cloud-only clip
    // still shows a real poster. The row and its `path` survive.
    lib.mark_evicted(uploaded).unwrap();
    let rec = lib.get(uploaded).unwrap().unwrap();
    assert!(rec.evicted);
    assert_eq!(rec.thumb_path.as_deref(), Some("done.jpg"));
    assert_eq!(rec.filmstrip_path.as_deref(), Some("done_strip.jpg"));
    assert_eq!(rec.path, "done.mp4");

    let (bytes_after, count_after) = lib.local_footprint().unwrap();
    assert_eq!((bytes_after, count_after), (1234, 1));
    // Already-evicted clips are no longer candidates.
    assert!(lib.evictable_clips().unwrap().is_empty());
    // The non-uploaded clip is untouched.
    assert!(!lib.get(local_only).unwrap().unwrap().evicted);
}

#[test]
fn custom_games_crud_and_dedupe() {
    let lib = Library::open_in_memory().unwrap();
    assert!(lib.list_custom_games().unwrap().is_empty());

    // Add normalizes the process name to lowercase.
    let g = lib
        .add_custom_game("GTA5.exe", "Grand Theft Auto V", None, None, Some("data:x"))
        .unwrap();
    assert_eq!(g.process_name, "gta5.exe");
    assert_eq!(g.display_name, "Grand Theft Auto V");
    assert!(g.enabled);
    assert_eq!(g.icon.as_deref(), Some("data:x"));
    assert_eq!(lib.enabled_custom_games().unwrap().len(), 1);

    // Re-adding the same exe (any case) refreshes the name + re-enables in
    // place — no duplicate row (Medal's per-machine custom DB is keyed by exe).
    lib.set_custom_game_enabled(g.id, false).unwrap();
    let g2 = lib
        .add_custom_game("gta5.exe", "GTA V", None, None, None)
        .unwrap();
    assert_eq!(g2.id, g.id);
    assert_eq!(g2.display_name, "GTA V");
    assert!(g2.enabled);
    // A re-add with no fresh icon keeps the previously captured one (COALESCE).
    assert_eq!(g2.icon.as_deref(), Some("data:x"));
    assert_eq!(lib.list_custom_games().unwrap().len(), 1);

    // Disabled entries stay listed but drop out of the detector's match list.
    lib.set_custom_game_enabled(g.id, false).unwrap();
    assert_eq!(lib.list_custom_games().unwrap().len(), 1);
    assert!(lib.enabled_custom_games().unwrap().is_empty());

    // Remove.
    lib.remove_custom_game(g.id).unwrap();
    assert!(lib.list_custom_games().unwrap().is_empty());
}
