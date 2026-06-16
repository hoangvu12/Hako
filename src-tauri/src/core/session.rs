//! Mode B full-match session writer + timeline index.
//!
//! While a match is INGAME, mux already-encoded packets to a temp file (no
//! re-encode) and record a PTS ↔ wall-clock index with round-boundary markers
//! for post-match kill reconciliation. Deleted after clips are cut.
