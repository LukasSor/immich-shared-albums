# Changes compared to upstream

This fork is based on [lukeet332/immich-shared-albums](https://github.com/lukeet332/immich-shared-albums). Everything below was added after commit `6b96129`; `git log 6b96129..main` lists each change as its own commit.

Nothing here changes how two servers find or trust each other. The additions to the wire protocol are optional fields and one optional route, which an upstream peer ignores or refuses. That has not been tested against a running upstream install.

## Container image

| | Upstream | This fork |
|---|---|---|
| Image | none published; build it from a clone (`docker build -t immich-shared-albums:live .`) | `ghcr.io/lukassor/immich-shared-albums:latest` |
| Tags | | `latest` (from `main`) and `sha-<short commit>` to pin one build |
| Built | by you | on every push to `main`, by `.github/workflows/image.yml` |
| Release workflows | release-please and a tag-driven release | removed; the image is the release |
| Compose and env | an example under `deploy/` | `docker-compose.yml` and `example.env` at the repo root, plus a Docker Compose section in the README |

```bash
docker pull ghcr.io/lukassor/immich-shared-albums:latest
```

## Fixes

Bugs present in upstream that are fixed here:

- **Uploads, mostly videos, failed with a 502 after exactly 30 seconds.** The proxy in front of Immich applied a 30 second read timeout to the whole exchange, including a request body still being sent, and Immich only answers once the whole body has arrived. Uploads now use a client with no read timeout; ordinary requests keep it (`97dd4f8`).
- **Duplicate mirror albums piled up.** Four separate causes:
  - leaving a share dropped the mapping even when Immich had failed to delete the album, leaving an orphan (`e5add51`);
  - the mapping was only saved after the members were added, so a failure in between lost track of an album that already existed (`8a3fd14`);
  - joining by link did not replace a dead mirror of the same share (`dd5c5cf`);
  - a link join and a background sync could both create the mirror of one share at the same moment (`a58e85c`).
- **Sync got stuck on photos your own household had contributed.** They came back through a share, could not be stubbed, and were retried for ever. They are now recorded as handled (`4352509`).
- **A reunion could pick the wrong album.** When two of your albums had the same name, the reunion took one of them. It now refuses unless exactly one of your own albums matches (`1b3ecd3`).
- **Stub thumbnails could stay blank.** The metadata written right after an upload started a second Immich job that could win the race against the one the upload queued. The metadata now travels inside the upload as an XMP sidecar, so only one job runs (`12a774d`).

## Features

- **An origin's changes now reach the mirror.** Upstream copies a name once and never again.
  - Album **rename** (`5f995f1`).
  - Album **description** and **cover** (`ec78d07`).
  - A photo's **place, rating and date**, not only its caption (`8c5d29c`).
  - A person's **profile picture**, re-checked at most hourly and uploaded only when it changed (`367813b`).
- **Duplicate albums, found and removed.** The panel's *Duplicate albums* section lists mirror albums that nothing tracks and that sit beside the live copy of the same share, then removes exactly that list after a confirmation. It only ever touches albums owned by the addon's own stand-in accounts (`1d8a97d`, `c11bfcb`).
- **A panel that looks like Immich.** The pages use Immich's palette, type, title bar, accordions, switches and icons, in light and dark (`96ec016`, `b5de72c`).
- **Postgres instead of SQLite, optional.** Set `ISA_DATABASE_URL` and the state lives in Postgres; a database that has never held a store takes a one-time copy of `state.db`, so the server keeps its identity and links. Moving back is explicit: `isa migrate-to-sqlite` copies Postgres into `state.db`, and an install that moved refuses to start on the stale `state.db` without it. SQLite stays the default. See [Using Postgres](deploy/configuration.md#using-postgres).
- **Edits from the other side, experimental.** With the switch below on, someone who can edit a shared album on a linked server can change its name, description or cover from their copy, and it reaches your album (`3551059`, `d53ad10`).

## New settings

Both are in the panel's Settings and both are **off by default**.

| Setting | What it does |
|---|---|
| Remove duplicate albums automatically | Deletes duplicate mirror albums in the background, at most five every ten minutes. The scan and the manual removal work either way. |
| Experimental: accept album edits from linked servers | Lets a linked server change the name, description or cover of an album you shared with edit rights. If you change the same thing yourself, your change wins. |

## Dependencies and security

- `minimatch` resolves to 11.x instead of 10.2.6, which has a denial of service in glob preprocessing. It is only used by the lint toolchain (`5744ed1`).
- `rustls` is 0.23.46 instead of 0.23.45, taking four advisories fixed in that release (`d318c0c`).
- The e2e suite logs fixed labels instead of values CodeQL mistook for secrets (`5da2196`).

## Staying close to upstream

The panel was restructured, so a merge of upstream's UI changes will conflict. Everything else is additive: new modules under `src/sync/` and `src/immich/`, new optional wire fields, and a few call sites in existing files.
