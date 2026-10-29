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
- The API is the contract; the TUI holds no state the server cannot rebuild.

### 1.2 Non-Goals (v1)

- Sync, collaboration, or mobile clients.
- Plugin system.
- Rich rendering of images, canvases, or embedded media; inline images only where the terminal supports them.
- Replacing a full editor; v1 ships a basic built-in editor (`tui-textarea`) plus `$EDITOR` integration.
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
| F5 | Backlinks panel for the selected note | Must |
| F6 | Fuzzy finder over note titles and aliases | Must |
| F7 | Full-text search with ranking and snippets | Must |
| F8 | Tag browser and filter | Should |
| F9 | Live reindex on external file changes | Should |
| F10 | Unlinked mentions (title appears in text but is not linked) | Should |
| F11 | Create note from a dangling link | Should |
| F12 | Daily notes | Could |
| F13 | Graph or "related notes" view | Could |
| F14 | Rename note and update inbound links | Could |
| F15 | Headless API usable without the TUI | Must |

## 3. Non-Functional Requirements

- **Performance:** cold start under 100 ms on an indexed source; full reindex of 10k notes under 10 s; search results under 50 ms; server round-trip under 5 ms on a local socket.
- **Safety:** atomic writes (write temp file, then rename); never delete user files without confirmation.
- **Portability:** no C toolchain required for end users; handle `\r\n` and `\n`; handle Windows path rules.
- **Robustness:** malformed frontmatter or invalid UTF-8 must not crash the app; the note is indexed best-effort and flagged. A client disconnecting mid-request must never affect the server or other clients.

---

## 4. Tech Stack

| Concern | Crate | Notes |
|---------|-------|-------|
| TUI rendering | `ratatui` | Immediate-mode widgets |
| Terminal backend | `crossterm` | Windows and Unix support |
| Text input widget | `tui-textarea` | Built-in editor and input fields |
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
│   ├── fuga-server/          # daemon: core + protocol + socket + watcher
│   │   └── src/
│   │       ├── main.rs
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
│   │           ├── preview.rs
│   │           ├── backlinks.rs
│   │           ├── finder.rs
│   │           ├── search.rs
│   │           └── editor.rs
│   └── fuga-cli/             # headless commands; also starts/stops the server
├── tests/fixtures/sources/   # sample notes for integration tests
└── docs/
```

**Principle:** `fuga-core` has zero knowledge of the terminal and of the socket. `fuga-server` has zero knowledge of the terminal. The TUI has zero knowledge of SQLite and the filesystem beyond spawning `$EDITOR`. This enables headless tests, headless scripting, and additional clients. `fuga-core` and `fuga-protocol` are published as reusable crates, so the API can be built on without the server or TUI.

### 5.3 Protocol

- **Framing:** newline-delimited JSON-RPC 2.0, one request or response per line. No HTTP, no streaming bodies; long results are chunked into paged responses.
- **Transport:** a local socket — Unix domain socket or Windows named pipe, via `interprocess`. Default path is derived from the config dir (`fuga.sock` / `fuga.pipe`). An optional TCP mode on `127.0.0.1` exists for debugging and tests only; it requires a token.
- **Access:** the local socket is created with owner-only permissions (0600). No authentication on the local socket; authentication (bearer token) is mandatory if TCP is enabled.
- **Versioning:** the server reports `api_version` on `handshake`; a client with a mismatched major version refuses to run and prints an upgrade hint. The request/response/event types live in their own crate (`fuga-protocol`) so a TUI/server change shows up as a reviewable diff there.
- **Errors:** JSON-RPC error objects with a stable `code` and a human `message`; never a panic. A malformed request line is answered with a parse error and the connection stays open.

Core methods (v1), all scoped by source where relevant:

| Method | Purpose |
|--------|---------|
| `handshake` | Version, capabilities, source list |
| `sources.list` | Configured sources and their state |
| `notes.list` | Paged notes for a source, with sort and filter |
| `notes.get` | Note body, frontmatter, flags, and resolved links |
| `notes.create` | New note (capture, daily, from dangling link) |
| `notes.write` | Atomic save of a note body |
| `notes.rename` | Rename plus inbound link rewrite |
| `links.backlinks` | Inbound links with line context |
| `links.unlinked` | Unlinked mentions of a title or alias |
| `search.fuzzy` | Ranked title/alias/path matches |
| `search.fulltext` | FTS5 query with snippets |
| `tags.list` / `tags.notes` | Tag browser |
| `index.status` / `index.rebuild` | Progress and forced rebuild |
| `events.subscribe` | Server pushes `note.changed`, `note.removed`, `index.progress` |

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
├── daily/
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

- **Identity:** a note is identified by `(source_id, path)`. New notes embed a timestamped `id` in the filename (`20261027T143012-rust-tui-stack.md`), so the id travels with the path; a frontmatter `id` is an optional secondary key for files that already carry one.
- If `id` is absent, the indexer assigns one and writes it back only if `auto_write_id = true` for that source; otherwise path identity is used and the file is never modified. Index and files stay in sync in both directions: a write-back updates the file, and an external edit of the `id` is picked up on the next scan.
- **Title resolution order:** frontmatter `title` → first `# H1` → filename stem.

### 6.3 Link Resolution

Links resolve within the note's own source unless the target is prefixed with a source name, e.g. `[[work:Index design]]`; a prefixed target resolves only in that source. Notes are linkable across sources.

For `[[target]]`, match case-insensitively in order:

1. Exact `id`
2. Exact title
3. Alias
4. Filename stem
5. Relative path (when the target contains `/`)

Ambiguity: choose the shortest path, flag the link as `ambiguous` in the index, and surface it in the UI (the preview marks the link and lists the candidates). Unresolved links are stored with `target_note_id = NULL` (dangling).

### 6.4 SQLite Schema

```sql
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
  enabled   INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE notes (
  id           INTEGER PRIMARY KEY,
  source_id    INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  uid          TEXT,             -- frontmatter id, nullable
  path         TEXT NOT NULL,    -- source-relative, '/' separated
  title        TEXT NOT NULL,
  body         TEXT NOT NULL,    -- markdown without frontmatter
  hash         BLOB NOT NULL,    -- blake3 of file bytes
  mtime_ns     INTEGER NOT NULL,
  size         INTEGER NOT NULL,
  created_at   INTEGER,
  parse_flags  INTEGER NOT NULL DEFAULT 0,  -- bit0: bad frontmatter, bit1: bad utf8
  UNIQUE (source_id, path)
);
CREATE UNIQUE INDEX idx_notes_uid ON notes(source_id, uid) WHERE uid IS NOT NULL;

CREATE TABLE aliases (
  note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  alias   TEXT NOT NULL,
  PRIMARY KEY (note_id, alias)
);
CREATE INDEX idx_aliases_alias ON aliases(alias COLLATE NOCASE);

CREATE TABLE tags (
  note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  tag     TEXT NOT NULL,
  PRIMARY KEY (note_id, tag)
);
CREATE INDEX idx_tags_tag ON tags(tag);

CREATE TABLE links (
  id             INTEGER PRIMARY KEY,
  source_id      INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  target_text    TEXT NOT NULL,   -- raw text inside [[ ]]
  target_id      INTEGER REFERENCES notes(id) ON DELETE SET NULL,
  heading        TEXT,
  display        TEXT,
  line           INTEGER NOT NULL,
  ambiguous      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_links_source ON links(source_id);
CREATE INDEX idx_links_target ON links(target_id);
CREATE INDEX idx_links_text   ON links(target_text COLLATE NOCASE);

CREATE VIRTUAL TABLE notes_fts USING fts5(
  title, body, tags,
  content = 'notes', content_rowid = 'id',
  tokenize = 'unicode61 remove_diacritics 2'
);
-- Triggers keep notes_fts in sync with notes (insert/update/delete).
```

**Search ranking:** `bm25(notes_fts, 10.0, 1.0, 5.0)` weights title, body, and tags; snippets via `snippet()`. Every query is filtered by `source_id`.

---

## 7. Indexing

### 7.1 Full Scan

1. Walk the source with `ignore` (skip hidden dirs, non-`.md`, and the configured `exclude` globs).
2. For each file, compare `(mtime_ns, size)` to the DB. If unchanged, skip.
3. If changed, read the file, compute `blake3`; if the hash is equal, update mtime only.
4. Otherwise parse and upsert note, aliases, tags, links in one transaction.
5. Delete DB rows whose files no longer exist.
6. **Second pass:** re-resolve all dangling and affected links (needed because a new note can satisfy old links).

Batch work in transactions of about 500 notes. Report progress through `index.progress` events.

### 7.2 Incremental Updates

- Watcher emits `Created | Modified | Removed | Renamed(from, to)` after a 100 to 200 ms debounce.
- Self-writes (saves made through the server) are tagged with an expected hash and ignored on echo.
- A removed-then-created pair within the debounce window is treated as a modify (common with editors that save via rename).
- Re-resolve links targeting the changed note's old and new title, aliases, and path.
- A source whose root no longer exists is marked unavailable, not deleted; its notes stay in the index until the source is removed from config.

### 7.3 Schema Migrations

`meta.schema_version` plus ordered migration functions. On a version mismatch that has no migration, drop and rebuild the index (it is a cache).

---

## 8. Parsing

Pipeline per file:

1. Decode UTF-8 (lossy on failure, set `parse_flags`).
2. Split frontmatter (`---` fenced block at file start); parse with `serde_yaml_ng` into a struct with all fields optional.
3. Run `pulldown-cmark` over the body, skipping code blocks and inline code.
4. A custom scanner over text events extracts:
   - `[[...]]` wikilinks (target, `#heading`, `|display`), with line numbers
   - `#tags` (word-boundary rules; exclude headings like `# Title`, URLs, and code)
5. Output a `ParsedNote` struct (pure data, no I/O), which makes the parser trivially unit- and property-testable.

---

## 9. Search

| Mode | Backend | Scope | Trigger |
|------|---------|-------|---------|
| Fuzzy find | `nucleo` over in-memory list of titles, aliases, paths | Fast jump to note | `Ctrl-P` |
| Full text | FTS5 `MATCH` with bm25 | Titles, bodies, tags | `/` |
| Tag filter | `tags` table | Notes by tag(s) | `t` |
| Backlinks | `links` by `target_id` | Inbound links with line context | Always visible |
| Unlinked mentions | FTS phrase query on title and aliases, minus linked notes | Suggest links | `u` |

Query syntax in full-text mode: plain words (implicit AND), `"phrases"`, `tag:rust`, `-exclude`. Parsed into an FTS5 expression with proper escaping to avoid FTS syntax errors from user input. Every search takes an optional source filter; with several sources configured the TUI defaults to all enabled sources.

Large sources (50k+ notes) are supported by never holding note bodies in memory: the fuzzy finder keeps only titles, aliases, and paths, and full-text search returns only the requested page of results and snippets, read on demand.

---

## 10. TUI Client Design

### 10.1 Layout

```
┌ Notes ─────────────┬ Preview ───────────────────────────┐
│ > Rust TUI stack   │ # Rust TUI stack                   │
│   SQLite FTS5      │ See SQLite FTS5 and schema ...     │
│   Index design     │                                    │
│   ...              ├ Backlinks (2) ─────────────────────┤
│                    │ Index design:14  "uses [[Rust..."  │
│                    │ Daily 2026-10-26:3                 │
├────────────────────┴────────────────────────────────────┤
│ NORMAL  notes/inbox/rust-tui.md   312 notes  indexed ✓  │
└─────────────────────────────────────────────────────────┘
```

Overlays: fuzzy finder (centered popup), search results, new-note prompt, help, server status.

### 10.2 Modes

`Normal` · `Insert` (built-in editor) · `Finder` · `Search` · `Prompt` · `Help`

### 10.3 Default Keymap (configurable)

| Key | Action |
|-----|--------|
| `j` / `k`, arrows | Move selection |
| `Enter` | Open or follow link |
| `Tab` | Cycle focus: list, preview, backlinks |
| `n` | Quick capture (new note in the source's `capture_dir`) |
| `Ctrl-P` | Fuzzy finder |
| `/` | Full-text search |
| `t` | Tag browser |
| `s` | Switch source |
| `i` | Edit in built-in editor (`tui-textarea`, default) |
| `Ctrl-E` | Edit in `$EDITOR` (mappable) |
| `Ctrl-O` / `Ctrl-I` | Navigation history back / forward |
| `gd` | Daily note |
| `?` | Help |
| `q` | Quit |

### 10.4 Server Connection and Startup

- On launch the TUI connects to the configured socket and calls `handshake`. If no server is running it starts one (`fuga server start`) and waits for the socket, with a visible "starting server" state and a timeout.
- If the connection drops, the TUI shows a disconnected banner, retries with backoff, and re-syncs from `handshake` plus a fresh list — it does not exit and does not lose unsaved input buffers.
- The TUI holds no authoritative state: every mutation is a server call, and every local view is refreshed from server events.

### 10.5 Editor Handling

The built-in editor (`tui-textarea`) is the default: it edits the note body in place and saves through `notes.write`, so it never grows beyond text, cursor movement, and save. `$EDITOR` is the escape hatch for everything else, on `Ctrl-E` (mappable): leave alternate screen, disable raw mode, spawn `$EDITOR` (fallback `nvim`, `vim`, `nano` on Unix; `notepad` on Windows), wait, restore terminal, and tell the server to reindex that file. Always restore the terminal in a panic hook and on `Drop` so a crash never leaves the terminal broken.

### 10.6 Markdown Preview

The preview renders whatever Markdown the terminal can express: headings, lists, blockquotes, code blocks, emphasis, tables, rules, and links (wikilinks shown with their resolved title). Inline images are rendered only where the terminal supports them; otherwise the alt text or path is shown. Rendering is a `pulldown-cmark` event pass over the same parse the indexer uses, so preview and index never disagree about what a note contains.

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
daily_dir = "daily"
auto_write_id = true
exclude = ["archive/**", "templates/**"]

[[sources]]
name = "work"
path = "~/work/docs"
capture_dir = "inbox"
auto_write_id = false
exclude = ["drafts/**"]

[server]
transport = "local"    # "local" socket, or "tcp" for debugging/tests only
socket = ""            # empty = default path under the config dir
tcp_addr = "127.0.0.1:7737"
token = ""             # required when transport = "tcp"

[client]
source = ""            # empty = all enabled sources

[editor]
command = ""           # empty = $EDITOR

[index]
watch = true
index_dir = ""         # empty = default data dir

[ui]
theme = "dark"
date_format = "%Y-%m-%d"
```

CLI flags override the config, and the config overrides defaults. A source whose `path` does not exist is reported as unavailable in the TUI status bar and skipped by indexing; it is never created implicitly.

---

## 12. Cross-Platform Notes

- Store paths in the DB as source-relative with `/`; convert at the FS boundary.
- Case-insensitive filesystems (Windows, macOS): compare link targets case-insensitively, but preserve original case on disk.
- Reserved Windows filenames (`CON`, `NUL`, etc.) and illegal characters must be sanitized when generating filenames from titles.
- Handle both `\n` and `\r\n` on read; preserve the file's existing line endings on write.
- `notify` uses `ReadDirectoryChangesW` on Windows and inotify on Linux; fall back to polling on network drives.
- Local IPC uses a Unix domain socket on Unix and a named pipe on Windows, both through `interprocess`; the socket path must stay under the platform's path length limit.
- Requires Windows Terminal (or any VT-capable terminal) for correct color and Unicode; legacy `cmd.exe` is unsupported.
- Release builds: `x86_64-unknown-linux-musl` (static) and `x86_64-pc-windows-msvc`, for both `fuga` (server/CLI) and `fuga-tui`.

---

## 13. Testing Strategy

| Level | Approach |
|-------|----------|
| Parser | Unit tests plus `proptest` (never panics on arbitrary input) |
| Indexer | Fixture sources in `tempfile`; assert DB state after create/modify/rename/delete, and after adding or removing a configured source |
| Link resolution | Table-driven tests for ambiguity, aliases, case, dangling links, and per-source scoping |
| Search | Golden queries with `insta` snapshots |
| Protocol | Round-trip serialization tests for every request/response type; malformed-input tests must not kill the connection |
| Server | In-process server on a temp socket; integration tests for dispatch, subscriptions, and concurrent clients |
| TUI | Render to `ratatui::backend::TestBackend`, snapshot buffers, with a fake client |
| Watcher | Integration test with real FS events and generous timeouts |
| CI | GitHub Actions matrix: ubuntu and windows, `cargo test`, `clippy -D warnings`, `fmt --check` |

---

## 14. Milestones

| # | Milestone | Deliverable |
|---|-----------|-------------|
| M0 | Skeleton | Workspace, CI, config with sources, terminal setup and teardown with panic hook |
| M1 | Core read path | Source walk, parser, SQLite schema, full index; CLI `fuga index` |
| M2 | Protocol | `fuga-protocol` types, local socket server, `fuga-client`, `fuga server start` |
| M3 | Browse | TUI connects to server: note list, preview, open in `$EDITOR`, reindex on return |
| M4 | Links | Wikilink resolution, backlinks panel, follow link, history |
| M5 | Search | Fuzzy finder and FTS search with snippets |
| M6 | Capture | Quick capture, dangling-link note creation, daily notes |
| M7 | Live | File watcher, incremental indexing, server events and client reconnection |
| M8 | Multi-source | Source switching, per-source config, cross-source list and search filters |
| M9 | Polish | Tags browser, unlinked mentions, themes, config, release builds, built-in editor (`tui-textarea`), node-per-heading style notes |
| M10+ | Stretch | Rename with link rewrite, graph view, additional clients |
