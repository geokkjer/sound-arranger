# Spec: LAN ListenBrainz Mirror

## Objective

Build a small, self-hosted web app for viewing `geokkjer`'s ListenBrainz listening history from a local mirror.

The app exists because the ListenBrainz web UI is broader than needed, has no desired dark theme, and direct use of the ListenBrainz API can hit rate limits. The local app should make recent scrobbles fast to browse while reducing API traffic.

## Goals

- Run locally in a container.
- Be reachable on the LAN by direct port access.
- Mirror ListenBrainz listens into a local SQLite database.
- Import historical listens from a local ListenBrainz export or dump on first run.
- Use the ListenBrainz API only for incremental updates after import.
- Serve a minimal dark Vue UI focused on recent scrobbles.
- Keep local data usable when ListenBrainz is down or rate limited.

## Non-Goals

- Multi-user support.
- Public internet hosting.
- Listen submission.
- Listen deletion from ListenBrainz.
- Album art in v1.
- Statistics dashboards in v1.
- Recommendations, playlists, or social features.
- Reverse proxy support in v1.
- Kubernetes manifests in v1, though the design should remain Kubernetes-friendly.

## User

The only target user for v1 is `geokkjer`.

The username should still be configured through an environment variable so the code does not hardcode the account in multiple places.

## Architecture

```text
LAN browser
  |
  | HTTP on direct LAN port
  v
Containerized Express app
  |
  | serves Vue static assets
  | exposes /api/* endpoints
  v
SQLite database on mounted volume
  |
  | first-run import from mounted dump/export
  | later incremental API sync
  v
ListenBrainz API
```

Use one container for v1. The Express backend serves both the built Vue frontend and the local JSON API.

## Tech Stack

- Frontend: Vue 3, Vite, TypeScript.
- Backend: Node.js, TypeScript, Express.
- Database: SQLite.
- Deployment: Docker Compose first.
- Future deployment: local Kubernetes with persistent volume storage.

## ListenBrainz API Facts

API root:

```text
https://api.listenbrainz.org
```

Recent listens endpoint:

```http
GET /1/user/{user_name}/listens
```

Useful query parameters:

- `count`: number of listens to return. Maximum documented value is `1000`.
- `min_ts`: return listens newer than this timestamp.
- `max_ts`: return listens older than this timestamp.

Rate-limit headers:

- `X-RateLimit-Limit`
- `X-RateLimit-Remaining`
- `X-RateLimit-Reset-In`
- `X-RateLimit-Reset`

The client must respect `429 Too Many Requests` responses and wait for the reset window before retrying.

## Data Import

### Primary Bootstrap Path

The first run should import listens from disk, not from the API.

Expected mounted paths:

```text
/app/import
/app/data
```

Preferred v1 input format:

```text
/app/import/listens/2024/1.listens
/app/import/listens/2024/2.listens
/app/import/listens/2025/1.listens
```

Each `.listens` file is JSON Lines: one ListenBrainz listen JSON document per line.

The app should support extracted `.listens` files first. Direct `.tar.zst` extraction can be added later if needed.

### Import Behavior

- Scan the configured import directory for `.listens` files.
- Read files line by line.
- Parse each line as JSON.
- Keep only listens for the configured user.
- Normalize each listen into the local schema.
- Insert in batches.
- Ignore duplicates.
- Store the raw JSON for future compatibility.
- Track import progress in the database.
- Leave existing database rows intact on restart.

### Import Trigger

On startup:

1. Open the SQLite database.
2. Run migrations.
3. Check if any listens exist.
4. If no listens exist and an import path exists, start import.
5. After import finishes, run incremental API sync.

The UI should also expose a manual import trigger for later re-imports or recovery.

## Incremental Sync

After the local database has listens, use ListenBrainz only for newer data.

Request shape:

```http
GET https://api.listenbrainz.org/1/user/geokkjer/listens?min_ts=<newest_local_listened_at>&count=1000
```

Sync should run:

- Once on startup after the database is ready.
- On a conservative interval, initially every 5 minutes.
- When the user clicks manual sync, if not cooling down.

Sync must not block the UI. The UI always reads from local SQLite through the backend API.

## API Backfill Fallback

Historical API backfill is a fallback, not the normal path.

Use API backfill only if:

- No local dump/export is available.
- Import fails and the user explicitly chooses API backfill.
- A future maintenance task needs to fill a known gap.

Fallback request shape:

```http
GET https://api.listenbrainz.org/1/user/geokkjer/listens?max_ts=<oldest_local_listened_at>&count=1000
```

The fallback must respect rate limits and be resumable.

## Rate-Limit Rules

- Parse rate-limit headers from every ListenBrainz response.
- Store the latest rate-limit state in `sync_state`.
- On `429`, stop ListenBrainz requests until the reset window expires.
- Manual sync must not bypass cooldown.
- Import from disk is not rate-limited and can continue independently.
- The frontend must never call ListenBrainz directly.

## Local API

### `GET /api/health`

Returns app and database health.

### `GET /api/listens?limit=50&before=<timestamp>`

Returns cached listens sorted newest first.

Parameters:

- `limit`: maximum number of listens to return.
- `before`: optional UNIX timestamp for pagination.

### `GET /api/sync/status`

Returns sync, import, and rate-limit status.

### `POST /api/sync`

Triggers incremental ListenBrainz sync if not cooling down.

### `POST /api/import`

Starts import from the configured import directory.

### `GET /api/import/status`

Returns import progress.

### `POST /api/import/pause`

Optional in v1. Pauses a long-running import if supported by the importer loop.

## Database Schema

### `listens`

```sql
CREATE TABLE listens (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  listened_at INTEGER NOT NULL,
  user_name TEXT NOT NULL,
  artist_name TEXT NOT NULL,
  track_name TEXT NOT NULL,
  release_name TEXT,
  recording_msid TEXT,
  recording_mbid TEXT,
  artist_mbids_json TEXT,
  duration_ms INTEGER,
  origin_url TEXT,
  music_service TEXT,
  dedupe_key TEXT NOT NULL UNIQUE,
  source TEXT NOT NULL,
  raw_json TEXT NOT NULL,
  inserted_at INTEGER NOT NULL
);
```

`source` values:

- `dump`
- `api`

### `sync_state`

```sql
CREATE TABLE sync_state (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
```

Useful keys:

- `newest_listened_at`
- `oldest_listened_at`
- `last_sync_started_at`
- `last_sync_finished_at`
- `last_sync_error`
- `rate_limit_remaining`
- `rate_limit_reset_at`
- `rate_limit_reset_in`

### `import_jobs`

```sql
CREATE TABLE import_jobs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  source_path TEXT NOT NULL,
  status TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  finished_at INTEGER,
  total_lines_seen INTEGER NOT NULL DEFAULT 0,
  total_inserted INTEGER NOT NULL DEFAULT 0,
  total_skipped INTEGER NOT NULL DEFAULT 0,
  last_error TEXT
);
```

`status` values:

- `pending`
- `running`
- `paused`
- `completed`
- `failed`

## Deduplication

Preferred dedupe key:

```text
listened_at + recording_msid
```

Fallback dedupe key:

```text
listened_at + artist_name + track_name
```

The normalized row should always include a final `dedupe_key` string so SQLite can enforce uniqueness.

## Frontend Requirements

V1 has one main screen.

The screen should include:

- App title.
- Recent listens list.
- Manual sync button.
- Import status.
- Sync status.
- Rate-limit cooldown status when active.
- Load older button.
- Empty state.
- Backend unavailable state.

Each listen row should show:

- Track name.
- Artist name.
- Release name when available.
- Relative listened time.
- Exact date and time in secondary text or tooltip.
- Optional music service text when available.

Design requirements:

- Dark theme by default.
- Compact list density.
- Responsive layout for desktop and mobile.
- No dashboard bloat.

## Container Requirements

Initial Docker Compose shape:

```yaml
services:
  listen:
    build: .
    ports:
      - "8080:8080"
    volumes:
      - ./data:/app/data
      - ./import:/app/import:ro
    environment:
      LISTENBRAINZ_USER: geokkjer
      DATABASE_PATH: /app/data/listens.sqlite
      IMPORT_PATH: /app/import
      PORT: 8080
```

The container should bind to `0.0.0.0` inside the container so Docker can expose it on the LAN host.

Public internet exposure is out of scope.

## Kubernetes Compatibility

Do not implement Kubernetes manifests in v1, but avoid choices that block a later move to local Kubernetes.

Future Kubernetes shape:

- One deployment for the app container.
- One persistent volume claim for SQLite data.
- Optional import volume or one-time import job.
- Service exposed only inside the LAN.
- Reverse proxy or ingress later if the solution proves useful.

## Commands

Expected development commands:

```bash
npm install
npm run dev
npm run build
npm run test
npm run lint
```

Expected container commands:

```bash
docker compose up --build
docker compose down
docker compose logs -f
```

Backup concept:

```bash
cp ./data/listens.sqlite ./backups/listens-$(date +%F).sqlite
```

## Project Structure

Target structure:

```text
frontend/
  src/
    api/
    components/
    composables/
    types/
backend/
  src/
    db/
    import/
    listenbrainz/
    routes/
    sync/
    types/
data/
import/
docs/
Dockerfile
docker-compose.yml
package.json
```

## Testing Strategy

Backend tests:

- Normalize ListenBrainz listen payloads.
- Generate stable dedupe keys.
- Insert duplicate listens safely.
- Import JSONL files line by line.
- Filter imported rows to the configured user.
- Build correct incremental sync URL.
- Parse and store rate-limit headers.
- Refuse sync during cooldown.

Frontend tests:

- Render listen rows.
- Render empty state.
- Render import status.
- Render sync status.
- Handle backend errors.

Manual checks:

- First run imports from `./import`.
- Restart does not duplicate listens.
- Recent listens load from SQLite without calling ListenBrainz.
- Manual sync fetches only newer listens.
- Container restart preserves `./data/listens.sqlite`.

## Success Criteria

- App runs through Docker Compose.
- UI is reachable from the LAN on port `8080`.
- SQLite database persists outside the container.
- First-run import loads listens from extracted `.listens` files.
- UI reads listens only from the local backend.
- Incremental sync uses `min_ts` and respects rate limits.
- Manual sync does not bypass cooldown.
- Recent listens page is dark, compact, and usable on mobile.

## Open Questions

- What exact user field appears in the chosen export format? The importer should be written with a small adapter once a sample file is available.
- Should v1 include a manual full re-import flow, or only first-run import?
- Should the app keep a separate import audit table per file for better resume behavior?
