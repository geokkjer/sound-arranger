# Implementation Plan: LAN ListenBrainz Mirror

## Overview

Build a containerized Node/TypeScript and Vue app that imports `geokkjer`'s ListenBrainz export from disk into SQLite, serves a dark recent-listens UI from local data, and performs safe incremental API syncs after import.

No implementation should start until this plan and the spec are accepted.

## Architecture Decisions

- Use Express because the backend API is small and should stay easy to understand.
- Use SQLite because this is a personal local mirror with simple persistence and easy file backups.
- Use one container because v1 does not need separate frontend/backend deployment units.
- Serve the built Vue app from Express to simplify LAN deployment.
- Import extracted `.listens` JSONL files first instead of supporting `.tar.zst` directly.
- Keep the frontend isolated from ListenBrainz. Only the backend talks to the upstream API.
- Keep Kubernetes support as a later deployment target, not a v1 deliverable.

## Phase 1: Project Skeleton

### Task 1: Create TypeScript Workspace

Description: Set up the repository structure for backend, frontend, shared scripts, and docs.

Acceptance criteria:

- Root package scripts exist for development, build, test, and lint.
- `backend/` and `frontend/` directories exist.
- TypeScript configuration is present.
- No application features are implemented yet.

Verification:

- Run `npm install`.
- Run `npm run build` and confirm the empty skeleton builds.

Likely files:

- `package.json`
- `tsconfig.json`
- `backend/tsconfig.json`
- `frontend/tsconfig.json`

Dependencies: None.

Estimated scope: Medium.

### Task 2: Add Express Health Server

Description: Add a minimal Express server with a health endpoint.

Acceptance criteria:

- `GET /api/health` returns JSON.
- Server reads `PORT` from the environment.
- Server binds to `0.0.0.0` for container compatibility.

Verification:

- Run the backend locally.
- Request `GET /api/health` and confirm a healthy response.

Likely files:

- `backend/src/server.ts`
- `backend/src/config.ts`

Dependencies: Task 1.

Estimated scope: Small.

### Task 3: Add Vue App Shell

Description: Add a Vite Vue app with a dark empty shell.

Acceptance criteria:

- Vue app runs in development.
- Dark theme baseline exists.
- App shows a planning placeholder or empty state.

Verification:

- Run frontend dev server.
- Open the app and confirm the dark shell renders.

Likely files:

- `frontend/src/main.ts`
- `frontend/src/App.vue`
- `frontend/src/styles.css`

Dependencies: Task 1.

Estimated scope: Small.

### Task 4: Add Docker Compose Skeleton

Description: Add container files that run the app and mount persistent data/import directories.

Acceptance criteria:

- `docker compose up --build` starts the app.
- Port `8080` is exposed.
- `./data` is mounted at `/app/data`.
- `./import` is mounted read-only at `/app/import`.

Verification:

- Run `docker compose up --build`.
- Request `GET /api/health` through the mapped port.

Likely files:

- `Dockerfile`
- `docker-compose.yml`
- `.dockerignore`

Dependencies: Tasks 2 and 3.

Estimated scope: Medium.

## Checkpoint: Foundation

- App starts locally.
- App starts in Docker Compose.
- Health endpoint works.
- Vue shell is visible.

## Phase 2: Database Layer

### Task 5: Add SQLite Connection And Migrations

Description: Add SQLite initialization and migrations for `listens`, `sync_state`, and `import_jobs`.

Acceptance criteria:

- Database path comes from `DATABASE_PATH`.
- Tables are created on startup.
- Database file persists under `/app/data` in the container.

Verification:

- Start the app.
- Confirm the SQLite file exists in `./data`.
- Run tests for migration creation.

Likely files:

- `backend/src/db/connection.ts`
- `backend/src/db/migrations.ts`
- `backend/src/db/schema.sql`

Dependencies: Task 2.

Estimated scope: Medium.

### Task 6: Add Listen Repository

Description: Add database helpers for inserting and querying normalized listens.

Acceptance criteria:

- Batch insert supports duplicate-safe writes.
- Recent listens can be queried newest first.
- Older listens can be queried with `before` pagination.
- Newest and oldest timestamps can be queried.

Verification:

- Unit test insert, duplicate insert, recent query, and pagination.

Likely files:

- `backend/src/db/listensRepository.ts`
- `backend/src/types/listen.ts`
- `backend/src/db/listensRepository.test.ts`

Dependencies: Task 5.

Estimated scope: Medium.

### Task 7: Add Sync State Repository

Description: Add key-value persistence helpers for sync and rate-limit metadata.

Acceptance criteria:

- Values can be set and read by key.
- Missing keys are handled safely.
- Repository supports sync metadata needed by later phases.

Verification:

- Unit test get, set, overwrite, and missing key behavior.

Likely files:

- `backend/src/db/syncStateRepository.ts`
- `backend/src/db/syncStateRepository.test.ts`

Dependencies: Task 5.

Estimated scope: Small.

## Checkpoint: Database

- Migrations run at startup.
- Listens can be inserted and queried.
- Sync state persists across restart.

## Phase 3: Importer

### Task 8: Add Listen Normalizer

Description: Convert ListenBrainz JSON into the local `Listen` row shape.

Acceptance criteria:

- Extracts listened timestamp, artist, track, release, MBIDs, duration, origin URL, and music service when present.
- Generates a stable dedupe key.
- Keeps raw JSON.
- Rejects rows missing required artist, track, or timestamp.

Verification:

- Unit test normal ListenBrainz payloads.
- Unit test missing optional metadata.
- Unit test dedupe fallback behavior.

Likely files:

- `backend/src/listenbrainz/normalizeListen.ts`
- `backend/src/listenbrainz/normalizeListen.test.ts`

Dependencies: Task 6.

Estimated scope: Medium.

### Task 9: Add JSONL File Scanner

Description: Find extracted `.listens` files under the configured import directory.

Acceptance criteria:

- Recursively scans `IMPORT_PATH`.
- Returns only `.listens` files.
- Sorts files deterministically.
- Handles a missing import directory without crashing.

Verification:

- Unit test scanner with nested fake paths.

Likely files:

- `backend/src/import/findListenFiles.ts`
- `backend/src/import/findListenFiles.test.ts`

Dependencies: Task 1.

Estimated scope: Small.

### Task 10: Add Import Job Runner

Description: Read `.listens` files line by line, filter to the configured user, normalize rows, and insert in batches.

Acceptance criteria:

- Imports extracted `.listens` JSONL files.
- Filters to `LISTENBRAINZ_USER`.
- Tracks lines seen, inserted, skipped, and errors.
- Uses batch inserts.
- Does not duplicate rows on repeated import.

Verification:

- Unit test import from a fixture file.
- Unit test repeated import.
- Manual test with a small sample export.

Likely files:

- `backend/src/import/importJob.ts`
- `backend/src/db/importJobsRepository.ts`
- `backend/src/import/importJob.test.ts`

Dependencies: Tasks 6, 7, 8, and 9.

Estimated scope: Medium.

### Task 11: Add Import Routes

Description: Expose import status and manual import trigger through the local API.

Acceptance criteria:

- `GET /api/import/status` returns latest import state.
- `POST /api/import` starts an import if one is not already running.
- Concurrent import attempts are rejected cleanly.

Verification:

- API tests or manual requests confirm status and trigger behavior.

Likely files:

- `backend/src/routes/importRoutes.ts`
- `backend/src/routes/index.ts`

Dependencies: Task 10.

Estimated scope: Small.

## Checkpoint: Import

- A local sample export imports into SQLite.
- Re-running import does not duplicate listens.
- Import status is visible through the API.

## Phase 4: ListenBrainz Incremental Sync

### Task 12: Add ListenBrainz API Client

Description: Add backend-only client for fetching newer listens and parsing rate-limit headers.

Acceptance criteria:

- Builds `min_ts` sync URLs correctly.
- Parses ListenBrainz response payloads.
- Parses rate-limit headers.
- Represents `429` as a cooldown result.

Verification:

- Unit test URL building.
- Unit test rate-limit parsing.
- Unit test response parsing with fixture JSON.

Likely files:

- `backend/src/listenbrainz/client.ts`
- `backend/src/listenbrainz/client.test.ts`

Dependencies: Task 8.

Estimated scope: Medium.

### Task 13: Add Incremental Sync Job

Description: Fetch listens newer than the local newest timestamp and store them in SQLite.

Acceptance criteria:

- Reads newest local timestamp.
- Fetches newer listens using `min_ts`.
- Inserts new listens with `source = api`.
- Updates sync status.
- Does nothing if currently in cooldown.

Verification:

- Unit test sync with mocked API client.
- Unit test cooldown refusal.

Likely files:

- `backend/src/sync/incrementalSync.ts`
- `backend/src/sync/incrementalSync.test.ts`

Dependencies: Tasks 7 and 12.

Estimated scope: Medium.

### Task 14: Add Sync Routes And Scheduler

Description: Expose sync controls and run periodic syncs.

Acceptance criteria:

- `GET /api/sync/status` returns latest sync and rate-limit state.
- `POST /api/sync` triggers sync if allowed.
- Startup sync runs after database initialization.
- Periodic sync runs every 5 minutes.

Verification:

- Manual request triggers sync.
- Logs show startup and periodic sync attempts.
- Cooldown blocks manual and scheduled sync.

Likely files:

- `backend/src/routes/syncRoutes.ts`
- `backend/src/sync/scheduler.ts`

Dependencies: Task 13.

Estimated scope: Medium.

## Checkpoint: Sync

- App can import local history.
- App can sync newer listens from ListenBrainz.
- Rate limits are respected.
- Frontend still does not call ListenBrainz.

## Phase 5: Local API For Listens

### Task 15: Add Listens Route

Description: Expose cached listens to the frontend.

Acceptance criteria:

- `GET /api/listens?limit=50` returns newest cached listens.
- `before` pagination returns older cached listens.
- Invalid query parameters return useful errors.

Verification:

- API test or manual request confirms pagination.

Likely files:

- `backend/src/routes/listensRoutes.ts`
- `backend/src/routes/listensRoutes.test.ts`

Dependencies: Task 6.

Estimated scope: Small.

## Phase 6: Frontend UI

### Task 16: Add Frontend API Client

Description: Add typed frontend helpers for local backend endpoints.

Acceptance criteria:

- Fetches listens.
- Fetches sync status.
- Fetches import status.
- Triggers manual sync.
- Handles backend errors.

Verification:

- Unit test API helpers with mocked fetch.

Likely files:

- `frontend/src/api/client.ts`
- `frontend/src/types/api.ts`

Dependencies: Tasks 11, 14, and 15.

Estimated scope: Small.

### Task 17: Build Recent Listens View

Description: Render the local listen history in a compact dark list.

Acceptance criteria:

- Shows track, artist, release, and listened time.
- Supports loading older listens.
- Shows empty state when no listens exist.
- Works on mobile and desktop widths.

Verification:

- Component tests for list, empty state, and load older.
- Manual browser check.

Likely files:

- `frontend/src/components/ListenList.vue`
- `frontend/src/components/ListenRow.vue`
- `frontend/src/App.vue`

Dependencies: Task 16.

Estimated scope: Medium.

### Task 18: Build Status And Controls

Description: Show import/sync/rate-limit state and provide manual sync control.

Acceptance criteria:

- Shows current import status.
- Shows latest sync result.
- Shows cooldown when active.
- Manual sync button calls local API.
- Button is disabled during cooldown or active sync.

Verification:

- Component tests for status states.
- Manual browser check with mocked or real backend states.

Likely files:

- `frontend/src/components/SyncStatus.vue`
- `frontend/src/components/ImportStatus.vue`
- `frontend/src/components/SyncControls.vue`

Dependencies: Task 16.

Estimated scope: Medium.

## Checkpoint: Usable V1

- UI displays local listens.
- User can load older listens.
- User can trigger safe manual sync.
- Import and sync status are visible.
- App remains usable while ListenBrainz is unavailable.

## Phase 7: Hardening And Operations

### Task 19: Add Logging

Description: Add clear logs for startup, import, sync, rate-limit cooldown, and errors.

Acceptance criteria:

- Logs show import start, progress, completion, and failure.
- Logs show sync start, inserted count, cooldown, and errors.
- Logs do not print secrets.

Verification:

- Run `docker compose logs -f` during import and sync.

Likely files:

- `backend/src/logger.ts`
- Import and sync files from earlier tasks.

Dependencies: Tasks 10 and 13.

Estimated scope: Small.

### Task 20: Add Backup And Restore Notes

Description: Document how to back up and restore the SQLite database.

Acceptance criteria:

- Docs explain where the database lives.
- Docs show backup command.
- Docs show restore command.
- Docs warn to stop the app before copying for safest backup.

Verification:

- Read docs and confirm commands match Docker Compose paths.

Likely files:

- `docs/OPERATIONS.md`
- `README.md`

Dependencies: Task 4.

Estimated scope: Small.

### Task 21: Add Final Verification Suite

Description: Ensure tests and build commands cover backend, frontend, and container packaging.

Acceptance criteria:

- `npm run test` passes.
- `npm run build` passes.
- `docker compose up --build` starts the app.
- Manual smoke test succeeds with a small import fixture.

Verification:

- Run all commands above.

Likely files:

- `package.json`
- test config files
- CI config only if explicitly requested later

Dependencies: All implementation tasks.

Estimated scope: Medium.

## Risks And Mitigations

| Risk | Impact | Mitigation |
| --- | ---: | --- |
| Export JSON shape differs from assumptions | High | Start importer with a small real sample file and adapt the normalizer before bulk import. |
| Large import blocks the event loop | Medium | Use line-by-line streaming and batch inserts. Avoid reading whole files into memory. |
| Duplicate detection misses edge cases | Medium | Store raw JSON and dedupe with the best available key. Add tests with real duplicate examples. |
| SQLite writes are interrupted | Medium | Use transactions per batch and keep import jobs resumable enough to rerun safely. |
| ListenBrainz rate limits still occur | Medium | Persist cooldown state and refuse scheduled/manual sync until reset. |
| Container exposes app beyond LAN | Medium | Keep deployment local, document direct port exposure, and defer reverse proxy/public auth. |
| SQLite on Kubernetes needs care | Low | Use a persistent volume and keep one app replica unless database strategy changes. |

## Parallelization Opportunities

- Frontend static UI can start after local API response shapes are defined.
- Database repositories and ListenBrainz client tests can be built independently after types are defined.
- Docker work can happen alongside backend skeleton work.
- Operations documentation can be written after paths and commands are stable.

## Sequential Dependencies

- Importer depends on database schema and normalizer.
- Incremental sync depends on normalizer, repositories, and rate-limit state.
- Frontend data rendering depends on local API response contracts.
- Kubernetes notes should wait until Docker Compose flow works.

## Definition Of Done For V1

- `docker compose up --build` starts the app.
- First-run import loads extracted `.listens` files from `./import`.
- SQLite data persists in `./data`.
- UI displays recent listens from SQLite.
- Manual sync fetches only listens newer than the local newest timestamp.
- Rate-limit cooldown is respected by scheduled and manual sync.
- App is usable from a LAN browser on port `8080`.
- Tests cover importer, dedupe, sync URL building, and cooldown behavior.
