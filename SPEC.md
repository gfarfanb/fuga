# fuga: Local-First Markdown Notes (Technical Spec)

**Status:** Draft v0.1
**Language:** Rust (stable)
**Targets:** Linux, Windows (Windows Terminal), macOS (best effort)
**Name:** `fuga`

---

## 1. Overview

fuga is a local-first note-taking system. Notes are plain Markdown files on disk, read from one or more configurable **sources**. A rebuildable SQLite index provides backlinks, tags, and full-text search.

The system is split in two processes:

- **`fuga-server`** — the main application. It owns the sources, the index, the watcher, and the search. It is a headless daemon and exposes a small local API.
- **`fuga-tui`** — the first-party client. A keyboard-driven terminal UI talking to the server over that API.

The TUI is the first client, not the product. Anything the TUI can do is a server operation, so a future GUI, editor plugin, or script uses the same API.

### 1.1 Goals

- Plain `.md` files are the single source of truth; the app never locks data in.
- Wikilink syntax: YAML frontmatter, `[[wikilinks]]`, `#tags`.
- Sources are configured, not hardcoded: any directory of Markdown files can be a source, and several can be active at once.
- Instant startup, fast search over thousands of notes.
- Single static binary per component, same behavior on Linux and Windows.
- Index is disposable: deleting it and rebuilding must always be safe.
- The API is the contract; the TUI owns no note data of its own, only view state (§10.1).

### 1.2 Non-Goals (v1)

- Sync, collaboration, or mobile clients.
- Plugin system.
- Rich rendering of images, canvases, or embedded media; inline images only where the terminal supports them.
- Replacing a full editor; v1 delegates all editing to `$EDITOR` and ships no built-in editor (§10.5).
- Writing back to a source in any format other than Markdown.
- Interoperability with any specific note app's proprietary features.

---

## 2. Functional Requirements

| ID | Requirement | Priority |
|----|-------------|----------|
| F1 | Configure one or more sources (directories of Markdown files) | Must |
| F2 | Quick capture: create a note from a single keystroke, with timestamped ID/title | Must |
| F3 | Parse frontmatter (title, aliases, tags, created) | Must |
| F4 | Parse `[[wikilinks]]`, `[[target\|alias]]`, `[[target#heading]]` | Must |
| F5 | Backlinks for the selected note, surfaced in the feed as related-by-reference (§10.1) | Must |
| F6 | Fuzzy finder over note titles and aliases | Must |
| F7 | Full-text search with ranking and snippets | Must |
| F8 | Criteria tree: sources, tags and filter terms, each toggled on and off to shape the note output | Should |
| F9 | Live reindex on external file changes | Should |
| F10 | Unlinked mentions (title appears in text but is not linked) | Should |
| F11 | Create note from a dangling link | Should |
| F12 | Restore the last session: criteria, query, output and position as the user left them; the idle output is the most recent note and its related notes | Must |
| F13 | Graph or "related notes" view | Could |
| F14 | Rename note and update inbound links | Could |
| F15 | Headless API usable without the TUI | Must |

## 3. Non-Functional Requirements

- **Performance:** cold start under 100 ms on an indexed source; full reindex of 10k notes under 10 s; search results under 50 ms; server round-trip under 5 ms on a local socket.
- **Safety:** atomic writes (write temp file, then rename); the server never deletes a file inside a source except the old path of a confirmed rename (§5.3) and its own case-probe file (§12). v1 has no note-delete method (delete is external: the user removes the file and the watcher drops the index row, §7.2).
- **Portability:** no C toolchain required for end users; handle `\r\n` and `\n`; handle Windows path rules.
- **Robustness:** malformed frontmatter or invalid UTF-8 must not crash the app; the note is indexed best-effort and flagged. A client disconnecting mid-request must never affect the server or other clients.

---

## 4. Tech Stack

| Concern | Crate | Notes |
|---------|-------|-------|
| TUI rendering | `ratatui` | Immediate-mode widgets |
| Terminal backend | `crossterm` | Windows and Unix support |
| Single-line text input | `tui-input` | The input line's search and new-note fields |
| Markdown parsing | `pulldown-cmark` | Event-based; wikilinks handled by a custom pass |
| Frontmatter | `serde` + `serde_yaml_ng` | Typed struct with permissive defaults |
| Database | `rusqlite` (feature `bundled`) | Bundles SQLite with FTS5; no system dependency |
| Fuzzy matching | `nucleo` | Same engine family as Helix |
| File walking | `ignore` | Respects `.gitignore`, fast |
| File watching | `notify` + `notify-debouncer-full` | Debounced events |
| Hashing | `blake3` | Change detection |
| CLI | `clap` | Subcommands and flags |
| Config | `serde` + `toml`, `directories` | Platform-correct config paths |
| Local IPC | `interprocess` | Unix domain socket and Windows named pipe |
| Serialization | `serde_json` | Request/response payloads |
| Errors | `thiserror` (libs), `anyhow` (bins) | |
| Logging | `tracing` + `tracing-appender` | Log to file, never to the TUI terminal |
| Testing | `insta`, `tempfile`, `proptest` | Snapshot, fixtures, parser fuzzing |

**Concurrency decision:** use plain OS threads and `crossbeam-channel` rather than `tokio`. The workload is file I/O, SQLite, and a local socket, not network services; this keeps the dependency graph and mental model small. One thread per client connection is enough for a handful of local clients.

**Editor decision:** v1 ships no built-in editor. In-place editing is `$EDITOR` (§10.5) and the single-line inputs (new-note prompt, search, fuzzy finder) are `tui-input` — a one-line field, not the multi-line textarea that is the TUI dependency most likely to lag `ratatui`. A future built-in editor (§14, M10+) stays in its own `views/` module, and a view names its widget crate only in that module, so adding or replacing one touches nothing else.

**Dependency pinning:** `fuga-tui` picks the `ratatui` major and `crossterm` follows it; the widget crate (`tui-input`) is chosen to fit that pair, never the reverse. Two `ratatui` majors can coexist in one build graph, but their types do not interoperate — a widget built against one cannot be handed a `Frame` or `Buffer` from the other — so the TUI's whole dependency set shares a single major.

---

## 5. Architecture

### 5.1 Processes and Layers

```
┌─────────────────────────┐   ┌─────────────────────────┐
│ fuga-tui (client)       │   │ future clients          │
│ views · widgets · keymap│   │ GUI · editor plugin ·   │
│                         │   │ scripts                 │
└───────────▲─────────────┘   └───────────▲─────────────┘
            │      fuga-client (typed API + transport)
            └──────────────┬──────────────┘
                           │ newline-delimited JSON-RPC over local socket
┌──────────────────────────┴───────────────────────────────┐
│ fuga-server  (main application, headless daemon)         │
│ ┌──────────────────────────────────────────────────────┐ │
│ │ Session layer: request dispatch, subscriptions,       │ │
│ │ per-connection state, event fan-out                   │ │
│ └───────────────────────▲──────────────────────────────┘ │
│ ┌───────────────────────┴──────────────────────────────┐ │
│ │ Core library (`fuga-core`)                           │ │
│ │ Source · Parser · Indexer · Search · Links           │ │
│ └───────┬───────────────────────┬──────────────────────┘ │
└─────────┼───────────────────────┼────────────────────────┘
          │                       │
┌─────────▼────────┐     ┌────────▼─────────┐
│ Filesystem       │     │ SQLite index     │
│ (.md files)      │     │ (rebuildable)    │
└─────────▲────────┘     └──────────────────┘
          │ notify events
┌─────────┴────────┐
│ Watcher thread   │
└──────────────────┘
```

**Process lifecycle:** the two processes are an implementation detail, not two things the user runs. On launch the TUI connects to the socket and, if nothing is listening, starts the server itself (see 10.4); `fuga server start --daemon` is for scripts, headless use, and service units.

### 5.2 Workspace Layout

```
fuga/
├── Cargo.toml                # workspace
├── crates/
│   ├── fuga-core/            # no UI and no IPC deps; fully testable
│   │   └── src/
│   │       ├── source.rs     # configured source roots, read/write, atomic save
│   │       ├── note.rs       # Note, NoteId, Frontmatter
│   │       ├── parser/
│   │       │   ├── mod.rs
│   │       │   ├── frontmatter.rs
│   │       │   ├── wikilink.rs
│   │       │   └── tags.rs
│   │       ├── index/
│   │       │   ├── schema.sql
│   │       │   ├── db.rs
│   │       │   ├── indexer.rs    # full + incremental
│   │       │   └── migrations.rs
│   │       ├── search.rs         # FTS + fuzzy
│   │       ├── links.rs          # resolution, backlinks, unlinked mentions
│   │       └── watcher.rs
│   ├── fuga-protocol/        # request/response/event types; no I/O
│   ├── fuga-server/          # daemon library: core + protocol + socket + watcher
│   │   └── src/
│   │       ├── lib.rs        # `fuga_server::run`; no main.rs — the binary is `fuga`
│   │       ├── server.rs     # listener, connection threads
│   │       ├── dispatch.rs   # method table -> core calls
│   │       └── events.rs     # change notifications to subscribers
│   ├── fuga-client/          # typed API client over the transport
│   │   └── src/{lib.rs,transport.rs,handle.rs}
│   ├── fuga-tui/             # ratatui client
│   │   └── src/
│   │       ├── main.rs
│   │       ├── app.rs        # state + update, mirrors server events
│   │       ├── event.rs      # input + server events
│   │       ├── keymap.rs
│   │       ├── theme.rs
│   │       └── views/
│   │           ├── list.rs
│   │           ├── criteria.rs
│   │           ├── preview.rs
│   │           ├── input.rs
│   │           ├── finder.rs
│   │           └── search.rs
│   └── fuga-cli/             # builds the `fuga` binary: `server start` + headless commands
├── tests/fixtures/sources/   # sample notes for integration tests
└── docs/
```

**Binaries:** exactly two ship — `fuga`, built from `fuga-cli` (which links `fuga-server` as a library and hosts both the `server` subcommands and the headless ones), and `fuga-tui`. `fuga-server` is a library crate with no `main.rs`, so the layout above is not a third binary. The release artifacts in §12 are those same two.

**Principle:** `fuga-core` has zero knowledge of the terminal and of the socket. `fuga-server` has zero knowledge of the terminal. The TUI has zero knowledge of SQLite and the filesystem beyond spawning `$EDITOR`. This enables headless tests, headless scripting, and additional clients. `fuga-core` and `fuga-protocol` are published as reusable crates, so the API can be built on without the server or TUI.

### 5.3 Protocol

- **Framing:** newline-delimited JSON-RPC 2.0, one request or response per line. No HTTP, no streaming bodies; long results are chunked into paged responses.
- **Transport:** a local socket — Unix domain socket or Windows named pipe, via `interprocess`. On Unix the default path is `$XDG_RUNTIME_DIR/fuga/fuga.sock` when `XDG_RUNTIME_DIR` is set, else `<config dir>/fuga.sock`; Windows uses the named pipe `\\.\pipe\fuga`. A path longer than `sun_path` (108 bytes on Linux, 104 on macOS — a deep home directory is enough) falls back to `<tmp>/fuga-<uid>/fuga.sock` in a 0700 directory, logged at startup, because no directory creation can shorten the config path; a `[server] socket` the user set too long is a config error rather than a silent override. Server and client resolve the path with the same function, so a client reaches the fallback too. An optional TCP mode on `127.0.0.1` exists for debugging and tests only; it requires a token.
- **Access:** the local socket is created with owner-only permissions (0600). No authentication on the local socket; authentication (bearer token) is mandatory if TCP is enabled.
- **Versioning:** the server reports `api_version` on `handshake`; a client with a mismatched major version refuses to run and prints an upgrade hint. The request/response/event types live in their own crate (`fuga-protocol`) so a TUI/server change shows up as a reviewable diff there.
- **Errors:** JSON-RPC error objects with a stable `code` and a human `message`; never a panic. A malformed request line is answered with a parse error and the connection stays open.

Core methods (v1), all scoped by source where relevant:

| Method | Purpose |
|--------|---------|
| `handshake` | Version, capabilities, source list |
| `sources.list` | Configured sources and their state |
| `notes.list` | Paged notes for a source, with sort and filter |
| `notes.get` | Note body, frontmatter, flags, resolved links (with byte spans), and the body's `base_hash` |
| `notes.create` | New note (capture, from dangling link) |
| `notes.write` | Atomic save of a note body, guarded by `base_hash` |
| `notes.rename` | Rename plus inbound link rewrite |
| `links.backlinks` | Inbound links with line context |
| `links.unlinked` | Unlinked mentions of a title or alias |
| `search.fuzzy` | Ranked title/alias/path matches |
| `search.fulltext` | FTS5 query with snippets |
| `notes.related` | Notes related to a note: inbound links first, then shared tags |
| `tags.list` / `tags.notes` | Tag browser |
| `index.status` / `index.rebuild` | Progress and forced rebuild |
| `index.reindex` | Reindex one note from disk now, past the fast path |
| `events.subscribe` | Server pushes `note.changed`, `note.removed`, `index.progress` |

- **Note keys:** a note-scoped method addresses a note by `{ source, path }` — the source's config name and the source-relative path as it is spelled on disk, the identity defined in §6.2. The lookup runs on `path_key` (§12), so on a case-insensitive source a client that spells the case differently still reaches the note, and the reply echoes the path as it is on disk. A client never sends `notes.id` (a surrogate row key that a rebuild re-elects) or `uid` (nullable, and re-elected for a duplicate). So `notes.get` and `notes.write` take that pair (`notes.write` also takes the body), and `notes.rename` takes the pair plus `new_path`, keeps the note's `uid`, and returns the note under its new path; `note.changed` and `note.removed` events carry the same pair.

- **Write conflicts:** `notes.get` returns the body's `base_hash` (the `blake3` the index stores, §6.4), and `notes.write` takes it back as an optional precondition. The server reads the file and compares before writing anything: on mismatch it writes nothing and answers with a `conflict` error carrying the current hash. The check and the temp-file write plus rename happen back to back in the same write path, so the residual window is a local race the spec accepts (the watcher catches any later change anyway). A write without `base_hash` is unconditional, last writer wins. Atomic rename alone never prevented this; it only prevents torn files.

- **Forced reindex.** `index.reindex` takes the note key (`{ source, path }`) and runs §7.1's pipeline for that one file with the `(mtime_ns, size)` fast path skipped, then re-resolves the links that target the note and emits `note.changed`; if the file is no longer there it drops the row and emits `note.removed`, exactly as the watcher would. The reply is empty. It is the forced form of the watcher's `Modified` path, for a client that wrote a file behind the server's back and knows it: `$EDITOR` (§10.5) is the case in v1, where the watcher may be off (`[index] watch = false`) or up to `poll_interval_ms` behind (§12). An unknown source is `not_found`; a disabled or missing one is `source_unavailable` (§11).

- **Related notes.** `notes.related` takes the note key (`{ source, path }`) plus `{ limit, cursor }` and returns other notes in two tiers: notes that link to it (`links.target_note_id`, §6.3) first, then notes sharing its `tag_key`s (§8.1), most shared first, ties broken by `mtime_ns` desc. The note itself is never returned, and like every search it spans all enabled sources unless a source filter is given. It is the feed's source when no query is active (§10.1); the two tiers are one ranked list, so one page can hold both.

- **Error codes.** JSON-RPC 2.0 reserves the framing codes and fuga adds nothing outside the implementation-defined server range (-32000..-32099). A client switches on `code`, never on `message`.

  | Code | Meaning | Notes |
  |------|---------|-------|
  | -32700 | parse error | The line was not JSON |
  | -32600 | invalid request | Not a JSON-RPC 2.0 request object |
  | -32601 | method not found | |
  | -32602 | invalid params | Also a cursor from another method or filter |
  | -32603 | internal error | A bug; logged, never a panic |
  | -32001 | `conflict` | `notes.write` `base_hash` mismatch (§5.3); `data.hash` is the current hash |
  | -32002 | `not_found` | Unknown note, source, or tag |
  | -32003 | `source_unavailable` | Root missing, or a write against a disabled source (§11) |
  | -32004 | `io_error` | Filesystem failure; `data.path` where known |

- **Pagination.** Every list-shaped reply (`notes.list`, `notes.related`, `search.fuzzy`, `search.fulltext`, `links.backlinks`, `links.unlinked`, `tags.list`, `tags.notes`) takes `{ limit, cursor }` and returns `{ items, next_cursor }`. `limit` defaults to 50 and is clamped to 1..=500; `cursor` is opaque, omitted or `null` for the first page, and `null` in the reply on the last page. A client never parses or constructs one, and a cursor from a different method or filter is `invalid params`. Opaque rather than an offset on purpose: `notes.list` and `search.fulltext` order by mtime and bm25 rank over an index the watcher mutates, so an offset would silently skip or repeat rows between pages.

- **Subscription filters.** `events.subscribe` takes `{ source?, events? }`, both optional and absent meaning all. `source` matches a `sources.name` exactly, the same name the note key uses above. `events` is drawn from the closed set `note.changed`, `note.removed`, `index.progress`. A filter narrows only that connection's stream; it is per-connection state (§5.4), replaced by the next `events.subscribe` and dies with the connection. There is no unsubscribe call and no per-note filter.

- **`index.progress` payload.** `{ source, done, total }`. `total` is the walk's best-effort count and may grow if a file appears mid-scan; `done == total` marks the pass complete. Emitted at most once per 500-note transaction (§7.1) plus once at completion. Like every event it is a hint: a client that misses one shows unknown progress, not a stuck one.

- **Events are a hint, not a log (the reconnect contract).** `events.subscribe` has no backlog and no resume cursor: the server does not buffer events for a client that is not connected, and there is no sequence number to resume from. A dropped connection therefore loses an unbounded number of `note.changed` / `note.removed` events, and no event stream is the source of truth for a client's state. Convergence is the client's job on reconnect: it re-fetches **every view it caches** — `handshake`, `notes.list`, and the preview and backlinks for the selected note, the related feed for the anchor note, and any open search results — because a fresh list alone leaves a stale body or stale backlinks displaying silently out-of-date content. Events arriving after a subscribe are applied on top of that resync and may be redundant. A client that loaded a note body keeps the `base_hash` it got with it and echoes it on `notes.write`, so a change it missed during the gap surfaces as a `conflict` on save instead of being silently overwritten. This is deliberate: at 50k notes a resume cursor would mean either server-side retention per client or a change journal, and a local single-writer app re-fetches titles in one paged call. The cost is the reconnect refetch; that cost is the contract.

### 5.4 Threading Model (server)

| Thread | Responsibility |
|--------|----------------|
| Listener | Accepts connections, spawns one thread per client |
| Client connections | Read requests, dispatch, write responses; own subscription filters |
| Indexer | Owns the write connection; consumes `IndexJob` messages |
| Watcher | Receives debounced FS events, converts to `IndexJob` |
| Search workers | Spawned per query; results tagged with a query ID so stale results are dropped |
| Event fan-out | Reads index results and pushes events to subscribed connections |

- Clients never block the indexer; requests are queued.
- SQLite runs in WAL mode: one writer (indexer), multiple readers (connections and search).
- The TUI's own event loop uses a tick (about 16 ms) plus a channel `select!` over input and server events.

---

## 6. Data Model

### 6.1 Sources on Disk

A source is any directory of Markdown files declared in the config. fuga does not require, create, or depend on any app-specific metadata inside a source, and it never assumes a source was produced by another note app.

```
~/notes/                 # source "notes"
├── inbox/
└── *.md

~/work/docs/             # source "work"
└── *.md
```

Indexes live outside the sources, under the app data dir, one file per source keyed by source name:

```
~/.local/share/fuga/index/notes.db
~/.local/share/fuga/index/work.db
```

This keeps sources clean and read-only-safe. Deleting an index file is always safe.

### 6.2 Note Format

```markdown
---
id: 20261027T143012
title: Rust TUI stack
aliases: [ratatui notes]
tags: [rust, tui]
created: 2026-10-27T14:30:12
---

# Rust TUI stack

See [[SQLite FTS5]] and [[Index design#Schema|schema]]. #draft
```

- **Identity:** a note is identified by `(source_id, path_key)` — the pair §5.3 sends as `{ source, path }`; `path` is the file's on-disk spelling, used for display and every filesystem call, and `path_key` is what uniqueness and lookups compare (§12). The integer `notes.id` is a surrogate row key and is never what a wikilink resolves to. A note also has at most one **note id**, a timestamped string such as `20261027T143012` (`NoteId` in `fuga-core`), stored in `notes.uid` and found through `idx_notes_uid`.
- **Where the note id comes from:** the filename when it begins with a timestamp (`20261027T143012-rust-tui-stack.md`) — that prefix is why the id travels with the path, so renaming a note keeps its id links valid — otherwise the frontmatter `id`, the secondary key for files that already carry one. If both are present and differ, the filename wins.
- If neither carries an id, the indexer assigns one and writes it back only if `auto_write_id = true` for that source; otherwise `uid` stays `NULL` and path identity is used and the file is never modified. Index and files stay in sync in both directions: a write-back updates the file, and an external edit of the `id` is picked up on the next scan.
- **Write-back preserves the rest of the file.** Frontmatter is *parsed* with `serde_yaml_ng` (§8), but never re-serialized: a round-trip would drop comments, reorder keys, and restyle quoting. The write-back is a raw-text edit of the existing `---` block — insert one `id: <value>` line, either after the opening `---` (if the block is otherwise empty) or as the last line of the block — and every other byte of the file is left untouched, including comments, key order, quoting, and `\r\n` vs `\n` line endings. If the file has no frontmatter block, one is prepended as `---\nid: <value>\n---\n`. If the block is malformed (bit0, §6.4), or already carries an `id`, the file is not written back. The edit goes through the atomic temp-file-plus-rename path (§3), so a crash never leaves a half-written note.
- A note id is unique per source (`idx_notes_uid`), so two notes sharing one are a copy, not an error: the path that sorts first keeps the id, the other keeps `uid = NULL` (path identity) and is flagged (bit2). Incremental updates never move an id between notes; a full rebuild re-elects.
- **Generating a note id.** When the server creates a note (quick capture, a note from a dangling link), the id is the local time at creation in the `YYYYMMDDTHHMMSS` form, walked forward one second at a time until it is free in the source — so the first capture in a second keeps that second's id and each later one takes the next free id. The target filename starts with the id (`<id>.md`, or `<id>-<title slug>.md`), so the same walk also skips a path that already exists on disk, and the create is exclusive: capture never replaces an existing file.
- **Title resolution order:** frontmatter `title` → first `# H1` → filename stem.
- **Tags:** frontmatter `tags` (a YAML sequence) and inline `#tags` merge per note by a canonical key (§8.1). The spelling is kept as written and is never normalized in the file.
- **Timestamps:** a note's time metadata is the frontmatter `created` (`created_at`, §8) and the file's modification time (`mtime_ns`); there is no daily note and no other note classification. The UI formats those two for display with `[ui].date_format` (§11), which never affects a note id, filename, or path.

### 6.3 Link Resolution

Links resolve within the note's own source unless the target is prefixed with a source name, e.g. `[[work:Index design]]`; a prefixed target resolves only in that source. Notes are linkable across sources.

For `[[target]]`, match case-insensitively in order:

1. Exact note id — `notes.uid`, the timestamped id indexed by `idx_notes_uid`, so `[[20261027T143012]]` is one index lookup and never a path scan
2. Exact title
3. Alias
4. Filename stem
5. Relative path (when the target contains `/`)

Every step is one index lookup — `idx_notes_uid`, `idx_notes_title`, `idx_aliases_alias`, `idx_notes_stem`, `idx_notes_path_nocase` (step 5 runs on `path_key` instead when the source folds case, §12) — so link resolution never scans `notes`, however many links a source has.

Ambiguity: choose the shortest path, flag the link as `ambiguous` in the index, and surface it in the UI (the preview marks the link and lists the candidates). Unresolved links are stored with `target_note_id = NULL` (dangling).

### 6.4 SQLite Schema

```sql
-- Per-connection setup: these run on every connection open, not once from
-- schema.sql (foreign_keys is not persistent; journal_mode=WAL is, but setting
-- it per connection is harmless). schema.sql itself holds DDL only.
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL            -- schema_version, last_full_scan
);

CREATE TABLE sources (
  id        INTEGER PRIMARY KEY,
  name      TEXT NOT NULL UNIQUE, -- config key
  root      TEXT NOT NULL,        -- absolute path at last index
  enabled   INTEGER NOT NULL DEFAULT 1,
  case_insensitive INTEGER NOT NULL DEFAULT 0  -- probed once on first full scan (§12)
);

CREATE TABLE notes (
  id           INTEGER PRIMARY KEY,
  source_id    INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  uid          TEXT,             -- note id: filename timestamp, else frontmatter id; nullable
  path         TEXT NOT NULL,    -- source-relative, '/' separated, on-disk casing (display and FS calls)
  path_key     TEXT NOT NULL,    -- identity: `path` byte for byte, or its NFC-normalized lowercase when the source's filesystem is case-insensitive (§12)
  stem         TEXT NOT NULL,    -- filename without directories and `.md`; derived for the title fallback (§6.2)
  title        TEXT NOT NULL,
  body         TEXT NOT NULL,    -- markdown without frontmatter
  hash         BLOB NOT NULL,    -- blake3 of file bytes
  mtime_ns     INTEGER NOT NULL,
  size         INTEGER NOT NULL,
  created_at   TEXT,             -- frontmatter `created`, ISO-8601 local time at second precision (§8); NULL when absent or unusable
  parse_flags  INTEGER NOT NULL DEFAULT 0,  -- bit0: bad frontmatter, bit1: bad utf8, bit2: duplicate note id
  UNIQUE (source_id, path_key)   -- not `path`: a CI source has one row per file whichever case the walk reports,
                                 -- while a case-sensitive source keeps `Notes/A.md` and `notes/a.md` as two notes
);
CREATE UNIQUE INDEX idx_notes_uid ON notes(source_id, uid) WHERE uid IS NOT NULL;
-- Resolution steps 2, 4 and 5 (§6.3). The query must compare with the same
-- COLLATE NOCASE, otherwise it is a per-link scan of the whole source.
-- NOCASE folds ASCII only, so non-ASCII titles/stems match case-sensitively.
CREATE INDEX idx_notes_title ON notes(source_id, title COLLATE NOCASE);
CREATE INDEX idx_notes_stem  ON notes(source_id, stem  COLLATE NOCASE);
CREATE INDEX idx_notes_path_nocase ON notes(source_id, path COLLATE NOCASE); -- step 5 (case-sensitive source); on a folding source, `path_key`

CREATE TABLE aliases (
  note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  alias   TEXT NOT NULL,
  PRIMARY KEY (note_id, alias)
);
CREATE INDEX idx_aliases_alias ON aliases(alias COLLATE NOCASE);

CREATE TABLE tags (
  note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  tag     TEXT NOT NULL,          -- spelling as written in that note (§8.1), for display
  tag_key TEXT NOT NULL,          -- canonical identity: case and separators folded (§8.1)
  PRIMARY KEY (note_id, tag_key)  -- not `tag`: `#Rust`, `#rust`, `#tagSeparated` are one tag
);
-- No COLLATE: `tag_key` is folded once at write time, so a plain B-tree is the
-- lookup the tag browser and the `tag:` filter need. `aliases` keeps its raw
-- spelling and matches with COLLATE NOCASE instead, because case is the only
-- fold it needs (§8.1).
CREATE INDEX idx_tags_key ON tags(tag_key);

CREATE TABLE links (
  id             INTEGER PRIMARY KEY,
  note_id        INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE, -- note containing the link
  target_text    TEXT NOT NULL,   -- raw text inside [[ ]]
  target_note_id INTEGER REFERENCES notes(id) ON DELETE SET NULL,
  heading        TEXT,
  display        TEXT,
  line           INTEGER NOT NULL,
  ambiguous      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_links_note   ON links(note_id);
CREATE INDEX idx_links_target ON links(target_note_id);
CREATE INDEX idx_links_text   ON links(target_text COLLATE NOCASE);

-- External-content FTS5 reads every indexed column from the content table, so
-- only columns of `notes` can be indexed. `tags` is many-to-many (tags table),
-- so it is not part of the FTS index; tag filtering is a join on `tags`.
CREATE VIRTUAL TABLE notes_fts USING fts5(
  title, body,
  content = 'notes', content_rowid = 'id',
  tokenize = 'unicode61 remove_diacritics 2'
);
-- Triggers keep notes_fts in sync with notes (insert/update/delete).
```

**Search ranking:** `bm25(notes_fts, 10.0, 1.0)` weights title and body; `snippet()` works because both indexed columns exist in `notes`. `tag:rust` is a join on `tags.tag_key` (§8.1), never an FTS term. Source filtering is always on `notes.source_id`; `aliases`, `tags` and `links` reach their source through the owning note (`JOIN notes n ON n.id = <table>.note_id`).

---

## 7. Indexing

### 7.1 Full Scan

1. Walk the source with `ignore` (skip hidden dirs, non-`.md`, and the configured `exclude` globs).
2. For each file, look it up by `path_key` (§12) and compare `(mtime_ns, size)` to the DB. If unchanged, skip.
3. If changed, read the file, compute `blake3`; if the hash is equal, update mtime only.
4. Otherwise parse and upsert note, aliases, tags, links in one transaction.
5. Delete the rows whose `path_key` the walk did not produce. Not "rows whose file no longer exists": `Path::exists` answers yes for a stored path that differs from the disk only in case or Unicode normal form, which is exactly the case this step has to clean up (§12). This pass runs only for a source whose root was read successfully: an unavailable root (§11) marks the source unavailable and deletes nothing, so its notes stay in the index (§7.2).
6. **Second pass:** re-resolve all dangling and affected links (needed because a new note can satisfy old links).

Batch work in transactions of about 500 notes. Report progress through `index.progress` events.

### 7.2 Incremental Updates

- Watcher emits `Created | Modified | Removed | Renamed(from, to)` after a 100 to 200 ms debounce. A rename that changes only case or Unicode normal form is the same `path_key` (§12): it rewrites `path` on the existing row and keeps `uid`, rather than inserting a second row for one file.
- Self-writes (saves made through the server, and the indexer's own `id` write-back, §6.2) are tagged with an expected hash and ignored on echo. A write-back stores the hash of the bytes it wrote, so its watcher echo fails §7.1 step 3's hash comparison, updates mtime only, and does not parse or write again.
- A removed-then-created pair within the debounce window is treated as a modify (common with editors that save via rename).
- Re-resolve links targeting the changed note's old and new note id, title, aliases, and path.
- A source whose root no longer exists is marked unavailable, not deleted; its notes stay in the index until the source is removed from config, which takes effect on the next server start (§11).

### 7.3 Schema Migrations

`meta.schema_version` plus ordered migration functions. On a version mismatch that has no migration, drop and rebuild the index (it is a cache).

---

## 8. Parsing

Pipeline per file:

1. Decode UTF-8 (lossy on failure, set `parse_flags`).
2. Split frontmatter (`---` fenced block at file start); parse with `serde_yaml_ng` into a struct with all fields optional.
   - `created`: accept a YAML date or local date-time — `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS`, fractional seconds dropped — and store it in that exact second-precision form, a date alone gaining `T00:00:00`. It is local time with no offset, so the column is `TEXT`, not an epoch integer: converting would mean guessing a timezone, and the fixed-width form sorts chronologically as text. Any other value (an offset, or unparseable) is ignored like any other bad optional field — `created_at` stays NULL, and `parse_flags` bit0 stays clear, since the frontmatter block itself is valid YAML.
3. Run `pulldown-cmark` over the body, skipping code blocks and inline code. The body goes through one entry point with one fixed `Options` set, and the scanner below is part of that entry point: the indexer (§7.1) and the preview (§10.6) call the same one, so neither can choose its own parser options, skip rules, or link spans.
4. A custom scanner over text events extracts:
   - `[[...]]` wikilinks (target, `#heading`, `|display`), with line numbers
   - `#tags` (word-boundary rules; exclude headings like `# Title`, URLs, and code)
5. Output a `ParsedNote` (pure data, no I/O) — the contract between the parser and the indexer: `frontmatter` (all fields optional), `body` (markdown without the frontmatter block), `aliases`, `tags` in document order, `links` (`target_text`, `heading`, `display`, `line`, and the byte span the scanner matched), and `parse_flags`. Pure data with no I/O is what makes the parser trivially unit- and property-testable, and it is what §7.1 step 4 upserts.

### 8.1 Tags

Frontmatter `tags` and inline `#tags` feed the same `tags` table and are merged per note: `ParsedNote` carries them in document order (frontmatter first, then body order), and two spellings of one canonical key collapse into a single row.

**Canonical key (`TagKey`, `fuga-core`).** Tags are compared by a key, not by their spelling, exactly as paths are compared by `path_key` (§12). `TagKey::new` takes the tag text (a leading `#` is dropped), NFC-normalizes it, then splits it into words:

- at every run of non-alphanumeric characters (`-`, `_`, space, `.`)
- before an uppercase letter that follows a lowercase letter or a digit (`tagSeparated` → `tag` `Separated`)
- before the last uppercase letter of an uppercase run followed by a lowercase letter (`HTTPServer` → `HTTP` `Server`)

Digits stay attached to the preceding word (`h264`, `a11y`). Each word is lowercased with `str::to_lowercase` — full Unicode, where SQLite's `NOCASE` folds ASCII only — and the words are joined with `-`. So `#Rust` and `#rust` are one key `rust`, and `#tagSeparated`, `#tag-separated` and `#tag_separated` are all one key `tag-separated`. A tag with no alphanumeric character is dropped.

**Why a key rather than `COLLATE NOCASE`:** case is only half the fold (separators, and Unicode case, are not `NOCASE`'s job), so it happens once at write time and every lookup compares the plain key. `aliases` needs case folding only, so it keeps the raw spelling and matches with `COLLATE NOCASE`. The two tables are deliberately not symmetric.

**Display and merge rules.** `tags.tag` keeps the spelling as written in that note — a tag is never rewritten in the file — and the first spelling in document order wins the row. `tags.list` returns one entry per `tag_key`, with a note count and the most frequent spelling (ties: lexicographically first). `tags.notes` and the `tag:` search term canonicalize their argument with the same function, so any spelling reaches the same tag.

---

## 9. Search

| Mode | Backend | Scope | Trigger |
|------|---------|-------|---------|
| Fuzzy find | `nucleo` over in-memory list of titles, aliases, paths | Fast jump to note | `Ctrl-S` to the fuzzy finder, or the input line in `Search` mode (§10.2) |
| Full text | FTS5 `MATCH` with bm25 | Titles, bodies (tag filter joins `tags`) | `Ctrl-S` to full-text, or a `/` query in the input line |
| Tag filter | `tags` table | Notes by tag(s) | a tag node in the Criteria tree (focus with `Shift-<arrow>`) |
| Backlinks | `links` by `target_note_id` | Inbound links with line context | Feed, related by reference (§10.1) |
| Unlinked mentions | FTS phrase query on title and aliases, minus linked notes | Suggest links | a Criteria tree node for the selected note |

Every filter above is a node in the TUI's Criteria tree (§10.1): source, tags, filter terms, excludes and sort. Toggling a node adds or removes that filter from the output. With no query active the output is the anchor note's related notes — inbound links first, then shared tags (`notes.related`, §5.3).

Query syntax in full-text mode: plain words (implicit AND), `"phrases"`, `tag:rust`, `-exclude`. `tag:` terms canonicalize to a `tag_key` (§8.1) and become a join on `tags`; the rest is parsed into an FTS5 expression with proper escaping to avoid FTS syntax errors from user input. Every search takes an optional source filter; with several sources configured the TUI defaults to all enabled sources.

Large sources (50k+ notes) are supported by never holding note bodies in memory: the fuzzy finder keeps only titles, aliases, and paths, and full-text search returns only the requested page of results and snippets, read on demand.

---

## 10. TUI Client Design

### 10.1 Layout

```
┌─Notes───────────────┬─Preview─────────────────────────────────────────────┐
│ > Rust TUI stack    │ # Rust TUI stack                                    │
│   SQLite FTS5       │ See SQLite FTS5 and schema ...                      │
│   Index design      │                                                     │
│                     │                                                     │
├─Criteria────────────┤                                                     │
│ ▾ Source            │                                                     │
│   [x] notes         │                                                     │
│ ▾ Tags              │                                                     │
│   [x] #rust         │                                                     │
│   [ ] #tui          │                                                     │
│ ▾ Filters           │                                                     │
│   [x] fts5          │                                                     │
├─────────────────────┴─────────────────────────────────────────────────────┤
│ Input fts                                                                 │
│ Search                                                Ctrl-Shift-' Help   │
├───────────────────────────────────────────────────────────────────────────┤
│ notes/inbox/rust-tui.md                      found 312 notes   server ✓   │
└───────────────────────────────────────────────────────────────────────────┘
```

Notes and Preview are two views of one ordered **output**: the Notes list is the navigation list by name, the Preview is the same sequence as a feed. What fills it:

- With a query active, the search results (§9): plain text is the fuzzy finder, `/` is full-text.
- With none, the **anchor** note's related notes — by reference first (inbound links), then by similar tags (`notes.related`, §5.3). The anchor is the last note opened (`Enter`); a session with nothing to restore starts from the most recently modified note.

The output is never held whole: it is paged in as the feed scrolls, and navigating or changing a criterion replaces or extends the sequence. Related notes are computed for the anchor note, not the marked one, so the sequence is frozen while the mark moves inside it; opening an entry re-anchors it to that note.

- **Notes** — the navigation list: one line per output entry by title, the current one marked (`>`). It opens on the last session's criteria, query, output and selection exactly as they were left; on a first run it starts from the most recently modified note and its related ones.
- **Criteria** — a tree of everything that shapes the output: source, tags, filter terms, excludes, sort, and later other modifiers. Any node can be enabled or disabled (`Space` on the focused node), and the enabled nodes compose the query behind the output: a source node is the source filter, a tag node joins on `tags.tag_key` (§8.1), a filter term goes into the FTS expression, an exclude node is the `-` side of it, and the sort node is the output order (all §9).
- **Preview** — full height, the output as a feed: one rendered note per entry (§10.6), paged in as it scrolls, and the entry it shows is the one marked in Notes.
- **Input** — always visible to the user, one line (`tui-input`, §4). The text goes to the active input mode (§10.2); `Enter` submits it and `Esc` clears it.
- **Input Options** — visually part of the input: the left side names the input mode, cycled with `Shift-Tab` (`Search`, or `Input` for a new note); the right side lists immediate commands, first of them `Ctrl-Shift-'` Help.
- **Status bar** — `notes/inbox/rust-tui.md   found 312 notes   <server-status>`: the selected note's path, the count of the current output, and the server state (starting §10.4, connected, disconnected, index progress, or an unavailable source §11). The input's label lives in Input Options, not here, and the server segment is also the way into the server-status overlay: `Ctrl-B`, or a click on it.

**Session state.** The view state — criteria tree and its enabled nodes, active query and input mode, output and its position, selected note, anchor note, scroll and feed position, input history — is written to `<data dir>/fuga/session.json` (debounced on change, and on exit) and read back at startup, so the app opens in the shape it was left, input mode included: the mode restored is the last one the user used, never a fixed one. It holds view state only, never note content: the results are the server's answer to the restored query, not a cached copy. An unreadable or unknown-version file is ignored and the defaults above apply, `Search` (§10.2) among them.

Overlays: help and server status. The fuzzy finder, the new-note prompt and the search-results overlay are gone — the input line, the Notes pane and the feed replace them — and the one-line server state moved to the status bar.

### 10.2 Modes

`Search` · `Input`

`Search` is the default and `Shift-Tab` cycles the two. There is no third mode and nothing is modal: the keymap (§10.3) acts while the input line is empty and the first printable key starts the text, and a mode shortcut selects a mode rather than a keystroke context — `Ctrl-S` cycles the `Search` sub-mode between the fuzzy finder over titles, aliases and paths and a full-text query (§9), and `Shift-Tab` selects `Input`, a capture in the source's `capture_dir`. `Enter` submits, leaving the line empty and ready for the next one, and a submitted or cancelled capture returns to `Search`; `Esc` clears the line. The active mode is named in Input Options and is session state, so a session opens with the mode it was left in, `Search` on a first run (§10.1).

Help and server status are overlays, not modes: `Ctrl-Shift-'` and `Ctrl-B` open them, `Esc` or the same key closes them, and the input line keeps its content underneath.

### 10.3 Default Keymap (configurable)

| Key | Action | Action id |
|-----|--------|-----------|
| `Enter` | Open or follow link; submit the input line | `open` |
| `Shift-<arrow>` | Move focus: notes, criteria, preview, input | `focus_panel` |
| `Shift-Tab` | Cycle input modes: `Search`, `Input` (config `backtab`) | `input_mode` |
| `Space` | Toggle the focused Criteria node on or off (only when the Criteria panel has focus) | `toggle_criterion` |
| `Esc` | Clear the input line, cancel a capture back to `Search`, close help or server status | `cancel` |
| `Ctrl-S` | Cycle `Search` mode: fuzzy finder, full-text search | `search_mode` |
| `Ctrl-E` | Edit in `$EDITOR` | `edit_external` |
| `Ctrl-Shift-'` | Help | `help` |
| `Ctrl-B` | Server status overlay, same as clicking the status-bar segment | `server_status` |
| `Ctrl-D` | Quit | `quit` |

`[keymap]` (§11) rebinds any of these by action id. A key string is `crossterm`'s spelling: a bare character (`e`), the prefixes `ctrl-`, `alt-`, `shift-` (`ctrl-e`, `alt-e`, `ctrl-shift-p`), and the named keys (`enter`, `esc`, `tab`, `backtab`, arrows, `f1`–`f12`). A string that does not parse, an unknown action id, or two actions on one key is a config error at startup.

Because the map is rebindable, the defaults only have to be safe, and `Ctrl-E` is safer than `Alt-E`: on macOS the Option key is not Meta by default, so `Option+E` gives `é` in Terminal.app and in a stock iTerm2 profile — an `Alt` default needs a per-terminal setting before it works at all, and one that looks broken is worse than one with a modifier. `Ctrl-E` needs nothing on any of the three targets; `e` or `alt-e` is one `[keymap]` line for anyone who prefers them.

### 10.4 Server Connection and Startup

- On launch the TUI connects to the configured socket and calls `handshake`. If no server is running it starts one (`fuga server start`) and waits for the socket, with a visible "starting server" state in the status bar and a timeout. On Windows the child is created with `CREATE_NO_WINDOW` (plus `DETACHED_PROCESS` for `--daemon`) and null stdio, so no console window flashes over the alternate screen and the child never writes into the TUI's console.
- If the connection drops, the TUI shows a disconnected banner, retries with backoff, and re-syncs from `handshake`, a fresh list, and a refetch of every cached view (body and backlinks for the selected note, the related feed for the anchor note) — it does not exit and does not lose unsaved input buffers. Events carry no backlog (§5.3), so this refetch, not the event stream, is what makes the client correct after a gap.
- The TUI holds no authoritative state: every mutation is a server call, and every local view is refreshed from server events. View state (criteria, output, position) is local and persisted (§10.1); note data never is.

### 10.5 Editor Handling

v1 has no built-in editor; editing is delegated to `$EDITOR`, on `Ctrl-E` (`[keymap]` in §11 rebinds it, and `[editor] command` overrides the binary). On that key: leave alternate screen, disable raw mode, spawn `$EDITOR` (fallback `nvim`, `vim`, `nano` on Unix; `notepad` on Windows), wait, restore terminal, and ask the server to reindex that file (`index.reindex`, §5.3) so the edit is visible on return instead of whenever the watcher fires. The external editor writes the file itself, so no save goes through `notes.write`; the `base_hash` and `conflict` machinery (§5.3) stays for API clients and for any later built-in editor. Always restore the terminal in a panic hook and on `Drop` so a crash never leaves the terminal broken.

No in-place editor on purpose: it would mean owning a text widget whose crate has to track `ratatui` (§4), and `$EDITOR` beats anything v1 could ship. A built-in editor is a later milestone (§14, M10+) and stays in its own `views/` module — no other module may name its widget crate — so it can be added, replaced, or dropped without touching the rest of the TUI.

### 10.6 Markdown Preview

The preview renders whatever Markdown the terminal can express: headings, lists, blockquotes, code blocks, emphasis, tables, rules, and links (wikilinks shown with their resolved title). Inline images are rendered only where the terminal supports them; otherwise the alt text or path is shown. Rendering is a `pulldown-cmark` event pass over the body from the same parse entry point the indexer uses — same `Options`, same scanner (§8) — with the resolved links and their byte spans from `notes.get` supplying the `[[…]]` spans it replaces with titles; preview and index therefore never disagree about what a note contains.

---

## 11. Configuration

Locations resolved with `directories`:

- Linux: `~/.config/fuga/config.toml`
- Windows: `%APPDATA%\fuga\config\config.toml`

```toml
# Each [[sources]] entry is an independent directory of Markdown files.
[[sources]]
name = "notes"
path = "~/notes"
capture_dir = "inbox"
auto_write_id = true
exclude = ["archive/**", "templates/**"]
watch_mode = "auto"        # "auto" | "native" | "poll"; see §12

[[sources]]
name = "work"
path = "~/work/docs"
capture_dir = "inbox"
auto_write_id = false
exclude = ["drafts/**"]

[server]
transport = "local"    # "local" socket, or "tcp" for debugging/tests only
socket = ""            # empty = default path (§5.3: XDG runtime dir, else the config dir)
tcp_addr = "127.0.0.1:7737"
token = ""             # required when transport = "tcp"

[client]
source = ""            # empty = all enabled sources

[editor]
command = ""           # empty = $EDITOR

[index]
watch = true
poll_interval_ms = 2000  # polling backend only (§12)
index_dir = ""         # empty = default data dir

[ui]
theme = "dark"
date_format = "%Y-%m-%d"  # display only: formats note timestamps; never an id, filename, or path

[keymap]
edit_external = "ctrl-e"  # action id (§10.3) = key string; unlisted actions keep their default
```

`[ui].date_format` is display-only: it formats the note timestamps the UI shows and never contributes to a note id, filename, or path (§6.2).

CLI flags override the config, and the config overrides defaults. A source whose `path` does not exist is reported as unavailable in the TUI status bar and skipped by indexing; it is never created implicitly — the root is the user's own location, and creating an empty directory there would hide an unmounted drive or a typo behind a source that looks merely empty.

The source's *subdirectories* are the opposite case: `capture_dir` is created on demand with `create_dir_all` the first time a note lands in it, and any parent directory of a path passed to `notes.create` (e.g. a note made from a dangling `[[folder/note]]` link) is created the same way, because the configured directory is where the note the user just asked for has to go. A create or capture against an unavailable source fails with the source-unavailable error and creates nothing.

Config is read once at startup. Adding or removing a source, or changing its options, is applied on the next `fuga server start`; v1 has no live reload, so there is no `sources.add` / `sources.remove` / `config.reload` method. A client that changes sources edits the file and restarts the server.

---

## 12. Cross-Platform Notes

- Store paths in the DB as source-relative with `/`; convert at the FS boundary.
- Case-insensitive filesystems (Windows, macOS): compare link targets case-insensitively, but preserve original case on disk. This reaches the index identity too. A source's filesystem is probed once, on its first full scan — create a hidden, non-`.md` probe file (`.fuga-case-probe`) in the root, look it up with its case flipped, then delete it immediately — and cached in `sources.case_insensitive`; the walk (§7.1 step 1) and the watcher (§7.2) ignore it, so it never surfaces as a note. A probe beats a platform guess because a macOS volume can be case-sensitive and an NTFS directory can opt in, and it beats a `GetVolumeInformationW` / `getattrlist` call because those answer per volume, not per directory, and because the probe keeps both outcomes testable on Linux (§13). For a case-insensitive source, `path_key` is `path` lowercased and NFC-normalized (macOS matches by normalization as well as case, so `Café.md` and the NFD spelling are one file) and lookups fold the same way; for a case-sensitive source `path_key` is `path` byte for byte. Only the key folds: `path` always keeps the on-disk spelling, and no path is ever rewritten. `str::to_lowercase` rather than SQLite's `NOCASE`, which folds ASCII only — `Ä` and `ä` are one file to Windows and macOS.
- Reserved Windows filenames (`CON`, `NUL`, etc.) and illegal characters must be sanitized when generating filenames from titles.
- Handle both `\n` and `\r\n` on read; preserve the file's existing line endings on write.
- `notify` uses `ReadDirectoryChangesW` on Windows and inotify on Linux. The native backend delivers nothing on a network filesystem and fails silently, so the watcher chooses the polling backend when detection says the source root is remote: Windows `GetDriveTypeW` returns `DRIVE_REMOTE`, or the path is a `\\` UNC path; Unix `statfs` reports NFS, CIFS/SMB, or FUSE. Because a FUSE or VM mount can lie about itself, the per-source `watch_mode` (`"auto"` default, or `"native"`/`"poll"` to override detection) wins, `[index].poll_interval_ms` (default 2000) sets the polling interval, and the chosen backend is logged at startup.
- Local IPC uses a Unix domain socket on Unix and a named pipe on Windows, both through `interprocess`. The pipe lives in the `\\.\pipe\` namespace, so it has no path length limit; the Unix socket path is resolved as in §5.3 and falls back to a short per-user path under the temp dir when the config dir is too deep.
- Requires Windows Terminal (or any VT-capable terminal) for correct color and Unicode; legacy `cmd.exe` is unsupported.
- Release builds: `x86_64-unknown-linux-musl` (static) and `x86_64-pc-windows-msvc`, for both `fuga` (server/CLI) and `fuga-tui`.

---

## 13. Testing Strategy

| Level | Approach |
|-------|----------|
| Parser | Unit tests plus `proptest` (never panics on arbitrary input); table-driven `TagKey` cases (case, camel/snake/kebab, acronyms, digits, Unicode, separator-only, frontmatter-plus-inline dedupe) |
| Indexer | Fixture sources in `tempfile`; assert DB state after create/modify/rename/delete of a file on disk (delete is external, §3), and after starting the indexer with a source added to or removed from the config (§11). Case folding is driven by the fixture's `case_insensitive` flag, so both sides are testable on Linux: a case-only path change leaves one row with its `uid` and no bit2 flag on a case-insensitive source, and keeps `Notes/A.md` plus `notes/a.md` as two notes on a case-sensitive one. One real-filesystem case-only rename on the Windows matrix leg. |
| Link resolution | Table-driven tests for ambiguity, aliases, case, dangling links, and per-source scoping |
| Search | Golden queries with `insta` snapshots; `tag:` spellings that must all hit the same tag |
| Protocol | Round-trip serialization tests for every request/response type; malformed-input tests must not kill the connection |
| Server | In-process server on a temp socket; integration tests for dispatch, subscriptions, and concurrent clients |
| TUI | Render to `ratatui::backend::TestBackend`, snapshot buffers, with a fake client; session save/restore round-trips, and a missing or corrupt session file falls back to defaults |
| Watcher | Integration test with real FS events and generous timeouts |
| CI | GitHub Actions matrix: ubuntu and windows, `cargo test`, `clippy -D warnings`, `fmt --check` |

---

## 14. Milestones

| # | Milestone | Deliverable |
|---|-----------|-------------|
| M0 | Skeleton | Workspace, CI, config with sources, terminal setup and teardown with panic hook |
| M1 | Core read path | Source walk, parser, SQLite schema, full index including link resolution (§6.3, §7.1); CLI `fuga index` |
| M2 | Protocol | `fuga-protocol` types, local socket server, `fuga-client`, `fuga server start` |
| M3 | Browse | TUI connects to server: layout (notes, criteria, preview, input, status bar), output and feed (related by reference and shared tags), session save/restore, open in `$EDITOR`, reindex on return |
| M4 | Links | Follow link from the preview; render resolved links and mark ambiguous or dangling ones (§10.6) |
| M5 | Search | Input line and modes, fuzzy finder, FTS search with snippets |
| M6 | Capture | Quick capture, dangling-link note creation |
| M7 | Live | File watcher, incremental indexing, server events and client reconnection |
| M8 | Multi-source | Source switching, per-source config, cross-source list and search filters |
| M9 | Polish | Tags and filter terms as criteria-tree nodes, unlinked mentions, themes, config, release builds, node-per-heading style notes |
| M10+ | Stretch | Rename with link rewrite, graph view, additional clients, built-in editor (§10.5) |
