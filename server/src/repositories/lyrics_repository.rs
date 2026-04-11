use anyhow::Result;
use chrono::prelude::*;
use indoc::indoc;
use rusqlite::{Connection, Transaction};

pub fn add_one(
    plain_lyrics: &Option<String>,
    synced_lyrics: &Option<String>,
    lyricsfile: &Option<String>,
    track_id: i64,
    instrumental: bool,
    source: &Option<String>,
    conn: &mut Connection,
) -> Result<i64> {
    let plain_lyrics = plain_lyrics.as_ref().filter(|s| !s.is_empty());
    let synced_lyrics = synced_lyrics.as_ref().filter(|s| !s.is_empty());
    let lyricsfile = lyricsfile.as_ref().filter(|s| !s.is_empty());

    let now = Utc::now();
    let query = indoc! {"
    INSERT INTO lyrics (
      plain_lyrics,
      synced_lyrics,
      lyricsfile,
      has_plain_lyrics,
      has_synced_lyrics,
      has_lyricsfile,
      instrumental,
      track_id,
      source,
      created_at,
      updated_at
    )
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
  "};
    let mut statement = conn.prepare(query)?;
    let row_id = statement.insert((
        plain_lyrics,
        synced_lyrics,
        lyricsfile,
        plain_lyrics.is_some(),
        synced_lyrics.is_some(),
        lyricsfile.is_some(),
        instrumental,
        track_id,
        source,
        now,
        now,
    ))?;
    Ok(row_id)
}

pub fn add_one_tx(
    plain_lyrics: &Option<String>,
    synced_lyrics: &Option<String>,
    lyricsfile: &Option<String>,
    track_id: i64,
    instrumental: bool,
    source: &Option<String>,
    conn: &mut Transaction,
) -> Result<i64> {
    let plain_lyrics = plain_lyrics.as_ref().filter(|s| !s.is_empty());
    let synced_lyrics = synced_lyrics.as_ref().filter(|s| !s.is_empty());
    let lyricsfile = lyricsfile.as_ref().filter(|s| !s.is_empty());

    let now = Utc::now();
    let query = indoc! {"
    INSERT INTO lyrics (
      plain_lyrics,
      synced_lyrics,
      lyricsfile,
      has_plain_lyrics,
      has_synced_lyrics,
      has_lyricsfile,
      instrumental,
      track_id,
      source,
      created_at,
      updated_at
    )
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
  "};
    let mut statement = conn.prepare(query)?;
    let row_id = statement.insert((
        plain_lyrics,
        synced_lyrics,
        lyricsfile,
        plain_lyrics.is_some(),
        synced_lyrics.is_some(),
        lyricsfile.is_some(),
        instrumental,
        track_id,
        source,
        now,
        now,
    ))?;
    Ok(row_id)
}

pub fn get_last_10_mins_lyrics_count(conn: &mut Connection) -> Result<i64> {
    let query = indoc! {"
    SELECT COUNT(*) FROM lyrics
    WHERE created_at > DATETIME('now', '-10 minute')
    AND source = 'lrclib'
  "};
    let mut statement = conn.prepare(query)?;
    let count = statement.query_row([], |row| row.get(0))?;
    Ok(count)
}
