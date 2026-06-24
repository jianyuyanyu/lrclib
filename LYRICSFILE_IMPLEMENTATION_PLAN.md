# Lyricsfile Integration Plan

## Goal

Add support for the new Lyricsfile format to LRCLIB without disrupting current behavior, caching, or database performance.

Near-term priorities:

- accept Lyricsfile in the publish API
- return Lyricsfile from get/search APIs
- preserve backward compatibility for existing clients
- avoid major schema changes and expensive migrations
- avoid harming hot read paths or cache behavior

Long-term direction:

- Lyricsfile should become the primary lyrics representation in LRCLIB
- this rollout should prepare for that future without forcing a large rewrite now

## Confirmed Product Decisions

1. Lyricsfile will be stored and returned as raw YAML.
2. If a client publishes Lyricsfile, it wins over `plainLyrics` and `syncedLyrics`.
3. When Lyricsfile is present in publish payload, legacy fields should be ignored.
4. Search cache may remain stale for performance reasons.

## Current Constraints

- SQLite database is very large, around 70GB.
- Request volume is high, up to roughly 600 requests per second.
- Current reads are optimized around `tracks.last_lyrics_id` joining to a single row in `lyrics`.
- `/api/get` has explicit cache invalidation on publish.
- `/api/search` is intentionally time-based cached and is not actively invalidated on publish.
- Current `search_cache` settings are 24h TTL and 4h idle timeout.

## Recommended Design

Use an additive design centered on one new nullable column in `lyrics`:

- add `lyricsfile TEXT NULL` to `lyrics`
- keep `plain_lyrics`, `synced_lyrics`, `instrumental`, and `tracks.last_lyrics_id` unchanged
- keep all current track matching and cache logic unchanged
- expose Lyricsfile as an optional additive response field

This is the smallest change that supports Lyricsfile end-to-end without introducing a new hot-path join, a backfill, or a schema redesign.

## Why This Design

### 1. Small database change

`ALTER TABLE lyrics ADD COLUMN lyricsfile TEXT` is much safer than introducing a new representation table or rebuilding existing rows.

### 2. No backfill required

Existing lyrics rows can keep `lyricsfile = NULL`.

### 3. Read path stays simple

Current read APIs already fetch one current lyrics row via `tracks.last_lyrics_id`. Adding one more nullable text column does not change the lookup pattern.

### 4. Cache behavior remains predictable

Current caches already store full typed responses. Adding an optional response field fits the existing design.

### 5. Backward compatibility remains intact

Existing clients can keep using `plainLyrics` and `syncedLyrics`. New clients can consume `lyricsfile`.

## Important Compatibility Rule

For this phase, if `lyricsfile` is present in a publish request:

- store `lyricsfile`
- ignore `plainLyrics`
- ignore `syncedLyrics`
- derive and store `plain_lyrics` and `synced_lyrics` from Lyricsfile for backward compatibility

This keeps request precedence simple while preserving compatibility for existing readers.

Implication:

- `lyricsfile` becomes the source of truth for that publish request
- older clients can still consume derived legacy fields

This gives a safer rollout path than trusting client-supplied legacy fields alongside Lyricsfile.

## Proposed API Changes

### Publish API

Extend `POST /api/publish` request body with an optional field:

- `lyricsfile: string | null`

Behavior:

- if `lyricsfile` is absent, keep current publish behavior unchanged
- if `lyricsfile` is present and non-empty, store it, ignore `plainLyrics` and `syncedLyrics` from the request, and derive legacy stored fields from Lyricsfile
- no strict Lyricsfile validation is required for now

### Get/Search APIs

Extend these response payloads with an optional field:

- `lyricsfile: string | null`

Endpoints:

- `GET /api/get`
- `GET /api/get/:track_id`
- `GET /api/search`

Keep all existing response fields unchanged.

## Database Plan

### Migration

Create a new migration that adds these fields:

```sql
ALTER TABLE lyrics ADD COLUMN lyricsfile TEXT;
ALTER TABLE lyrics ADD COLUMN has_lyricsfile BOOLEAN;
```

Then add an index:

```sql
CREATE INDEX idx_lyrics_has_lyricsfile ON lyrics (has_lyricsfile);
```

Recommended write rule:

- set `has_lyricsfile = true` when `lyricsfile` is present and non-empty
- set `has_lyricsfile = false` otherwise

### What not to do in this phase

- do not backfill existing rows
- do not introduce a separate `lyrics_representations` table

Reasoning:

- all hot lookups already use the current lyrics row directly
- `has_lyricsfile` gives an efficient future path for counting and filtering without scanning text payloads
- the extra boolean column and index add some write cost, but this is still much smaller than a schema redesign

Operational note:

- on a very large SQLite database, creating the new index may take noticeable time during migration
- this is still reasonable if you expect future operational queries like `COUNT(*) WHERE has_lyricsfile = 1`
- if migration time becomes a concern, the index can be deferred to a later migration, but adding it now is valid

## Code Changes

### 1. Publish path

Update `server/src/routes/publish_lyrics.rs`:

- add optional `lyricsfile` field to `PublishRequest`
- if `lyricsfile` exists and is non-empty:
  - ignore `plain_lyrics`
  - ignore `synced_lyrics`
  - derive `plain_lyrics` from `plain` when present, otherwise best-effort from `lines[*].text`
  - derive `synced_lyrics` best-effort from `lines[*].start_ms` plus `lines[*].text`
  - derive `instrumental` best-effort from `metadata.instrumental`
- define how `instrumental` is handled for Lyricsfile publishes

Recommended behavior for this phase:

- use a minimal best-effort YAML extraction from `metadata.instrumental`

Better option, still low-risk:

- add a very small best-effort parser for `metadata.instrumental`, `plain`, and `lines`
- do not perform full schema validation
- if parsing fails, still store raw `lyricsfile` and leave derived legacy fields empty

This preserves the existing legacy fields without requiring complete Lyricsfile validation.

### 2. Lyrics repository

Update `server/src/repositories/lyrics_repository.rs`:

- accept `lyricsfile` as an optional field on insert
- accept `has_lyricsfile` on insert
- persist it in both `add_one` and `add_one_tx`

### 3. Domain/entity mapping

Update `server/src/entities/lyrics.rs`:

- add `lyricsfile: Option<String>` to `Lyrics`
- add `lyricsfile: Option<String>` to `SimpleLyrics`

### 4. Track repository reads

Update `server/src/repositories/track_repository.rs`:

- include `lyrics.lyricsfile` in `SELECT` clauses for:
  - `get_track_by_id`
  - `get_track_by_metadata`
  - `get_tracks_by_keyword`
- map it into `SimpleLyrics`

### 5. API response structs

Update response structs in:

- `server/src/routes/get_lyrics_by_metadata.rs`
- `server/src/routes/get_lyrics_by_track_id.rs`
- `server/src/routes/search_lyrics.rs`

Add:

- `lyricsfile: Option<String>` serialized exactly as `lyricsfile`

## Cache Plan

### `/api/get` metadata cache

Current state:

- cache values are typed response objects
- cache keys are versioned with `get:v2`
- publish invalidates cached entries by `track_id`

Plan:

- keep invalidation logic unchanged
- bump cache key version from `get:v2` to `get:v3`

Reason:

- response shape is changing
- version bump prevents mixing old cached payloads with new response expectations

### `/api/search` cache

Current state:

- cached for 24h TTL
- 4h idle timeout
- no publish-triggered invalidation
- stale entries may be refreshed in the background when old

Plan:

- keep the current time-based strategy
- add a cache key version prefix for safety if needed during implementation
- do not add publish-triggered invalidation

Reason:

- `/search` is a hot endpoint
- immediate consistency is less important here than cache effectiveness

### Missing-track cache

No changes required.

### Challenge cache

No changes required.

## Validation Plan

Validation of Lyricsfile format is intentionally minimal in this phase.

Allowed approach:

- accept raw YAML payload as opaque text
- only do small optional extraction if needed for `instrumental`

Do not do in this phase:

- strict schema validation
- timestamp ordering validation
- line/word consistency checks
- YAML normalization or rewriting

## Rollout Phases

### Phase 1: Additive support

Deliverables:

- DB columns for `lyricsfile` and `has_lyricsfile`
- publish accepts `lyricsfile`
- get/search return `lyricsfile`
- existing publish/get/search behavior remains intact for legacy payloads
- metadata cache version bump

This phase is enough to support Lyricsfile in production with minimal risk.

### Phase 2: Better metadata extraction from Lyricsfile

Optional follow-up:

- best-effort extraction of `instrumental`
- potentially extract or confirm other useful metadata if ever needed

Still avoid full validation.

### Phase 3: Move Lyricsfile toward primary internal representation

Future work, not part of this rollout:

- define canonical behavior when both legacy and Lyricsfile data exist historically
- decide whether legacy fields should eventually be derived from Lyricsfile
- decide whether `/api/get` matching preference should consider Lyricsfile presence, not just `synced_lyrics`
- decide whether new clients should have Lyricsfile-first response contracts

## Edge Cases To Handle

1. Empty `lyricsfile`

- treat empty string like absent data and fall back to current legacy behavior

2. `lyricsfile` plus legacy fields together

- Lyricsfile wins
- legacy fields are ignored

3. Old rows without Lyricsfile

- continue returning `lyricsfile: null`

4. Search cache staleness

- newly published Lyricsfile content may not appear in cached `/api/search` results immediately
- this is accepted by design

5. Instrumental tracks

- if published as legacy synced lyrics, existing instrumental marker logic remains unchanged
- if published as Lyricsfile, instrumental handling should come from a small optional parse of `metadata.instrumental`, or default to `false` in the first pass

## Testing Plan

### Publish tests

- publish legacy-only payload and verify no regression
- publish Lyricsfile-only payload and verify row is stored
- publish payload containing Lyricsfile and legacy fields and verify Lyricsfile wins
- publish Lyricsfile payload with empty string and verify legacy fallback behavior

### Read tests

- `GET /api/get` returns `lyricsfile` when present
- `GET /api/get/:track_id` returns `lyricsfile` when present
- `GET /api/search` returns `lyricsfile` when present
- old rows still return legacy fields normally

### Cache tests

- publish invalidates `/api/get` metadata cache for that track
- `/api/search` remains cached and may be stale until natural refresh/expiry
- cache key version bump avoids mixing old and new response payloads

### Migration tests

- migration applies cleanly on an existing database
- existing rows remain readable without backfill

## Recommended Implementation Order

1. add migration for `lyricsfile`, `has_lyricsfile`, and its index
2. update lyrics insert repository functions
3. update publish request model and publish logic
4. update entity structs and repository row mapping
5. update get/search response structs
6. bump metadata cache key version
7. optionally version `/search` cache key
8. add tests for publish, get, search, and cache behavior
9. update architecture/docs after code lands

## Non-Goals For This Phase

- making Lyricsfile the only stored format
- converting old rows to Lyricsfile
- full Lyricsfile validation
- new search features over Lyricsfile content
- invalidating `/api/search` on every publish
- introducing a separate normalized representation table

## Final Recommendation

Implement Lyricsfile support as a strictly additive third representation stored on the existing `lyrics` row.

For the first rollout:

- accept raw YAML Lyricsfile in publish under `lyricsfile`
- return raw YAML Lyricsfile in get/search under `lyricsfile`
- ignore legacy fields when Lyricsfile is present
- avoid schema redesign and backfill
- keep `/search` cache strategy unchanged
- version `/api/get` cache keys to avoid response-shape mixing

This gives LRCLIB a safe path toward making Lyricsfile the primary format later, without taking on high migration risk or hot-path complexity today.
