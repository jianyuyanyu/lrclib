# Architecture

## Overview

Rust workspace for the LRCLIB API server.

- Root crate: thin CLI wrapper in `src/main.rs`.
- Main app: `server/` crate.
- Optional queue crate: `lrclib-queue-stub/` behind the root `queue` feature.
- Storage: SQLite via `rusqlite` + `r2d2`.
- HTTP: Axum.

Most real behavior lives in `server/`.

## Workspace Layout

- `src/main.rs`: parses `serve --port --database --workers-count`, then calls `server::serve(...)` or `server::serve_with_queue(...)`.
- `server/src/lib.rs`: app bootstrap, `AppState`, router, middleware, background tasks.
- `server/src/routes/*.rs`: HTTP handlers.
- `server/src/repositories/*.rs`: SQL access layer.
- `server/src/entities/*.rs`: small DB/domain structs.
- `server/src/utils.rs`: input normalization, publish-token verification, cache helpers.
- `server/migrations/*`: embedded SQLite schema/migrations, applied on startup.

## Runtime Flow

Server startup in `server::serve_with_queue`:

- init tracing from `LRCLIB_LOG`
- open SQLite pool
- apply PRAGMAs + migrations
- build shared `AppState`
- spawn background tasks
- build Axum router
- serve on `0.0.0.0:<port>` with graceful shutdown

Background tasks:

- request metrics reporter: logs requests/minute
- recent lyrics counter: refreshes `last 10 minutes` publish count once/minute
- queue supervisor: watches desired worker count and delegates to queue backend

## AppState

`AppState` is the shared runtime container.

- `pool`: SQLite connection pool.
- `challenge_cache`: proof-of-work publish challenges, TTL 5 min.
- `get_cache`: dedupe for missing-track enqueue requests, TTL 7 days.
- `get_metadata_cache`: cached `/api/get` responses.
- `get_metadata_index`: `track_id -> cache keys`, used to invalidate metadata cache on publish.
- `search_cache`: cached `/api/search` responses, 24h TTL, 4h idle timeout.
- `queue`: bounded in-memory `ArrayQueue<MissingTrack>`.
- `request_counter`, `recent_lyrics_count`: lightweight metrics.
- `workers_count`, `workers_tx`: runtime queue config.

## Layering

There is no separate service layer.

- route handlers validate + normalize input
- handlers read/write caches when needed
- handlers call repository functions directly
- repositories contain SQL and row mapping

If you need API behavior changes, start in `routes/`.
If you need query/schema changes, start in `repositories/` and `migrations/`.

## API Surface

Routes are mounted under `/api` in `server/src/lib.rs`.

- `GET /api/get`: lookup by metadata.
- `GET /api/get/:track_id`: lookup by track id.
- `GET /api/search`: FTS-backed search.
- `POST /api/request-challenge`: create proof-of-work challenge.
- `POST /api/publish`: publish lyrics using challenge solution.
- `POST /api/flag`: flag current lyrics for a track.
- `POST /api/manage/set-config`: update queue worker count.

## Main Request Paths

### `GET /api/get`

- Normalizes input with `utils::prepare_input`.
- Uses `get_metadata_cache` with rounded-duration buckets and +/-2s tolerance.
- Cache keys are versioned as `get:v3`.
- On cache miss, queries `track_repository::get_track_by_metadata`.
- Returns the track joined with `tracks.last_lyrics_id`.
- If not found and `album_name + duration` are present, creates a `MissingTrack` and pushes it to the in-memory queue.
- Missing-track enqueue is deduped through `get_cache`.

### `GET /api/search`

- Normalizes query params.
- Uses `search_cache`.
- Cache keys are versioned as `search:v2`.
- Repository searches `tracks_fts` (SQLite FTS5), then joins `tracks` + `lyrics`.
- Cached results may be refreshed in the background when old.

### `POST /api/request-challenge`

- Generates a random prefix.
- Computes difficulty from recent publish volume.
- Stores `challenge:<prefix> -> target` in `challenge_cache`.

### `POST /api/publish`

- Requires `X-Publish-Token`.
- Token format is `prefix:nonce` and is verified against the cached challenge by SHA-256 threshold comparison.
- In one DB transaction, find or insert the track, then insert a new lyrics row.
- DB trigger updates `tracks.last_lyrics_id`.
- If `lyricsfile` is supplied, it is stored as raw YAML, legacy `plainLyrics` / `syncedLyrics` inputs are ignored, and stored legacy columns are derived best-effort from the Lyricsfile payload.
- If only synced lyrics are supplied, plain lyrics are derived by stripping timestamps.
- Instrumental markers (`[au: instrumental]`) produce a lyrics row with no text and `instrumental = true`.
- For `lyricsfile` publishes, `instrumental` is populated by a best-effort YAML read of `metadata.instrumental`.
- After commit, invalidates metadata cache entries for that track id.

### `POST /api/flag`

- Reuses the same publish-token validation.
- Inserts into `flags` for the track's current `last_lyrics_id`.

### `POST /api/manage/set-config`

- Requires bearer token matching `LRCLIB_MANAGE_TOKEN`.
- Uses constant-time comparison.
- Updates desired queue worker count through a `watch` channel.

## Data Model

Core schema is in `server/migrations/`.

- `tracks`: canonical track metadata plus normalized `*_lower` columns and `last_lyrics_id`.
- `lyrics`: lyrics versions linked to a track; latest version is selected through `tracks.last_lyrics_id`. It now also stores optional raw `lyricsfile` YAML plus `has_lyricsfile` for efficient presence/count queries.
- `flags`: reports against a lyrics row.
- `missing_tracks`: deduped record of missing requests.
- `tracks_fts`: FTS5 virtual table over normalized track fields.

Important DB behavior:

- `tracks.last_lyrics_id` is maintained by an `AFTER INSERT ON lyrics` trigger.
- FTS rows are maintained by insert/update/delete triggers on `tracks`.
- track uniqueness is enforced on normalized `(name_lower, artist_name_lower, album_name_lower, duration)`.
- DB opens in WAL mode with tuned PRAGMAs in `server/src/db.rs`.
- `idx_lyrics_has_lyricsfile` supports efficient counts and filters over Lyricsfile adoption.

Current lyrics API responses remain backward-compatible:

- `plainLyrics` and `syncedLyrics` are still returned for legacy rows
- `lyricsfile` is returned as an additional raw YAML field when present

## Matching Rules

Track matching is intentionally fuzzy but narrow:

- names are normalized by `utils::prepare_input`
- punctuation is mostly removed/collapsed
- comparisons are lowercase
- metadata duration matching uses about +/-2 seconds
- `/api/get` prefers tracks whose current lyrics include synced lyrics

If you change matching behavior, review both:

- `utils::prepare_input`
- repository metadata queries and FTS usage

## Queue Status

Queue plumbing exists, but the current repo only ships stub backends.

- `server/src/queue_stub.rs`: default in-tree stub
- `lrclib-queue-stub/`: optional feature-backed stub crate

Both currently only log worker updates; they do not process `MissingTrack` items. The app can enqueue missing tracks, but this repo does not implement the scraper/consumer side.

## Agent Notes

- Most edits belong in `server/`, not the root crate.
- Preserve write transactions around track + lyrics creation.
- If publish behavior changes, remember metadata cache invalidation.
- Search cache is time-based only; publish does not actively invalidate it.
- Schema changes require a new migration and usually repository updates.
