use crate::{
    errors::ApiError,
    repositories::{lyrics_repository, track_repository},
    utils::{invalidate_get_metadata_cache_for_track_id, is_valid_publish_token, strip_timestamp},
    AppState,
};
use anyhow::Result;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use axum_macros::debug_handler;
use regex::Regex;
use rusqlite::Connection;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PublishRequest {
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: f64,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    lyricsfile: Option<String>,
}

#[derive(Deserialize, Default)]
struct LyricsfileDocument {
    metadata: Option<LyricsfileMetadata>,
    lines: Option<Vec<LyricsfileLine>>,
    plain: Option<String>,
}

#[derive(Deserialize, Default)]
struct LyricsfileMetadata {
    instrumental: Option<bool>,
}

#[derive(Deserialize, Default)]
struct LyricsfileLine {
    text: Option<String>,
    start_ms: Option<u64>,
}

#[debug_handler]
pub async fn route(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PublishRequest>,
) -> Result<StatusCode, ApiError> {
    match headers.get("X-Publish-Token") {
        Some(publish_token) => {
            let is_valid =
                is_valid_publish_token(publish_token.to_str()?, &state.challenge_cache).await;

            if is_valid {
                {
                    let mut conn = state.pool.get()?;
                    let track_id = publish_lyrics(&payload, &mut conn)?;

                    invalidate_get_metadata_cache_for_track_id(&state, track_id).await;
                }

                Ok(StatusCode::CREATED)
            } else {
                Err(ApiError::IncorrectPublishTokenError)
            }
        }
        None => Err(ApiError::IncorrectPublishTokenError),
    }
}

fn publish_lyrics(payload: &PublishRequest, conn: &mut Connection) -> Result<i64> {
    let mut tx = conn.transaction()?;
    let duration = payload.duration.round();

    let existing_track = track_repository::get_track_id_by_metadata_tx(
        &payload.track_name.trim(),
        &payload.artist_name.trim(),
        &payload.album_name.trim(),
        duration,
        &mut tx,
    )?;

    let track_id = match existing_track {
        Some(track_id) => track_id,
        None => track_repository::add_one_tx(
            &payload.track_name.trim(),
            &payload.artist_name.trim(),
            &payload.album_name.trim(),
            duration,
            &mut tx,
        )?,
    };

    let lyricsfile = payload
        .lyricsfile
        .as_ref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned());

    let (plain_lyrics, synced_lyrics, is_instrumental) =
        if let Some(lyricsfile) = lyricsfile.as_deref() {
            let document = parse_lyricsfile(lyricsfile);
            (
                derive_plain_lyrics(document.as_ref()),
                derive_synced_lyrics(document.as_ref()),
                document
                    .as_ref()
                    .and_then(|document| document.metadata.as_ref())
                    .and_then(|metadata| metadata.instrumental)
                    .unwrap_or(false),
            )
        } else {
            let mut plain_lyrics = payload
                .plain_lyrics
                .as_ref()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_owned());
            let synced_lyrics = payload
                .synced_lyrics
                .as_ref()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_owned());

            // Generate plain_lyrics from synced_lyrics
            if plain_lyrics.is_none() && synced_lyrics.is_some() {
                plain_lyrics = Some(strip_timestamp(synced_lyrics.as_deref().unwrap()));
            }

            // Create a regex to match "[au: instrumental]" or "[au:instrumental]"
            let re = Regex::new(r"\[au:\s*instrumental\]").expect("Invalid regex");
            let is_instrumental = synced_lyrics
                .as_ref()
                .map_or(false, |lyrics| re.is_match(lyrics));

            (plain_lyrics, synced_lyrics, is_instrumental)
        };

    if is_instrumental && lyricsfile.is_none() {
        // Mark the track as instrumental
        lyrics_repository::add_one_tx(
            &None,
            &None,
            &lyricsfile,
            track_id,
            true,
            &Some("lrclib".to_owned()),
            &mut tx,
        )?;
    } else {
        lyrics_repository::add_one_tx(
            &plain_lyrics,
            &synced_lyrics,
            &lyricsfile,
            track_id,
            false,
            &Some("lrclib".to_owned()),
            &mut tx,
        )?;
    }

    tx.commit()?;

    Ok(track_id)
}

fn parse_lyricsfile(lyricsfile: &str) -> Option<LyricsfileDocument> {
    serde_yaml::from_str::<LyricsfileDocument>(lyricsfile).ok()
}

fn derive_plain_lyrics(document: Option<&LyricsfileDocument>) -> Option<String> {
    let document = document?;

    if let Some(plain) = document.plain.as_ref().filter(|plain| !plain.is_empty()) {
        return Some(plain.to_owned());
    }

    let lines = document.lines.as_ref()?;
    let plain = lines
        .iter()
        .filter_map(|line| line.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n");

    (!plain.is_empty()).then_some(plain)
}

fn derive_synced_lyrics(document: Option<&LyricsfileDocument>) -> Option<String> {
    let lines = document?.lines.as_ref()?;
    let synced = lines
        .iter()
        .filter_map(|line| {
            let start_ms = line.start_ms?;
            let text = line.text.as_deref()?;

            Some(format!("{}{}", format_timestamp(start_ms), text))
        })
        .collect::<Vec<_>>()
        .join("\n");

    (!synced.is_empty()).then_some(synced)
}

fn format_timestamp(start_ms: u64) -> String {
    let total_seconds = start_ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let centiseconds = (start_ms % 1000) / 10;

    format!("[{minutes:02}:{seconds:02}.{centiseconds:02}]")
}
