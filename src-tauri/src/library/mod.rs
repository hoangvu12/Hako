//! Clip library: SQLite metadata + thumbnail extraction.

#![allow(dead_code)]

pub mod db; // SQLite (rusqlite): clips, tags, paths
pub mod thumbs; // thumbnail extraction (ffmpeg)
pub mod trim; // loss-less stream-copy trimming (ffmpeg)
