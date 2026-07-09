# Design doc for refactoring the `Note` struct and how the `Vault` interacts with notes

## Motivation

The `body` field on the `Note` type and the two different collections of `Note` types that the `Vault`
holds (`note_overrides` and `cached_disk_notes`) are confusing, complicate the implementation of various
features, and cause redundant copies of a note's content — most visibly in search, where cached notes are
re-parsed from their raw text (`Note::parse(path, text)`) whenever a content filter is active.

The goal of this refactor is a single, unambiguous representation of a note's contents and a single
collection of notes on the `Vault`.

## Source of truth

`Note.text` is the **single source of truth** for a note's contents. All other content-derived fields —
`id`, `title`, `aliases`, `tags`, `links`, `frontmatter`, `frontmatter_line_count` — are parsed from `text`
and are treated as a derived cache of it.

The invariant is: **after any mutation, `text` and the derived fields agree.** Any method that changes a
note's contents (e.g. `update_content`, `set_field`, `add_tag`, `remove_tag`) must re-serialize into `text`
and recompute the derived fields (reusing the existing `update_content` → reparse machinery).
`text` should not be `public` to avoid the situation where callers mutate it directly.

## Plan

### On the `Note` struct

- Replace the `Note.body: Option<String>` field with `Note.text: String` which holds the entire, exact
  contents of the note *including* the frontmatter. It is always populated.
- Add a `Note::body(&self) -> &str` accessor that returns the frontmatter-stripped body as a zero-copy
  slice of `text`. This reuses the existing `raw_note_sections()` / `frontmatter_line_count` logic, which
  already slices raw text into `(frontmatter, body)` consistently with `gray_matter`'s parsing. All body-only
  consumers (content search filters, `read(false)`, `merge`, `rename_tag`, extraction) go through `body()`.
- Remove `Note::from_path_with_body()` and make `Note::from_path()` always load the contents. This is a
  memory cost, not an I/O cost: `from_path()` already reads the whole file and then discards the body, so the
  only change is retaining the `String`.
- Remove the now-redundant body-management API: `load_body()`, `reload_with_body()`, the `keep_body`
  parameter on `parse_impl`, and `NoteError::BodyNotLoaded`.
- `write()` no longer requires an explicit "body loaded" check and can no longer fail with `BodyNotLoaded`.
- `write_frontmatter()` currently re-reads the body from disk to avoid clobbering on-disk body edits. With
  `text` in memory this read is unnecessary; `write_frontmatter()` will reconstruct from the in-memory
  `text`. (Note: this changes behavior for the CLI `update` path if the on-disk body was edited out of band
  after the note was loaded — acceptable given the in-memory `text` is now authoritative.)

Content search filters (`and_content_contains`, regex matches, etc.) must match against `body()`, **not**
`text` — otherwise queries would start matching against YAML frontmatter, a silent behavior change.

### On the `Vault` struct

- Replace the `Vault.note_overrides` and `Vault.cached_disk_notes` fields with a single field:
  `Vault.cached_notes: Option<HashMap<PathBuf, Arc<Note>>>`, and remove the `CachedDiskNote` type and its
  `load_cached_disk_note` helper.
- The `Option` is a **mode switch**, preserving today's `cached_disk_notes` semantics:
  - `None` — disk-walk mode (the CLI case). Searches and scans walk the filesystem. There are no in-memory
    overrides in this mode.
  - `Some(map)` — the map is the **authoritative, complete in-memory snapshot** of the vault. Searches and
    scans iterate the map only and never touch the filesystem. In-memory overrides and unsaved notes are
    simply entries in this map.
- In-memory overrides collapse into this single map: `load_note()` inserts/replaces an entry,
  `unload_note()` restores the on-disk version by re-reading it (`refresh_cached_note`), and
  `note_is_loaded()` / `has_cached_note()` query the map. `load_note()` requires `Some` (snapshot) mode;
  overriding a note only makes sense against a snapshot.
- Entries are `Arc<Note>` for cheap sharing. `note_for_path()` continues to return an owned `Note` (clone).

### In the `obsidian_core::search` API

- Remove `CachedSearchNote`. With `Note.text` always present, search iterates `Arc<Note>` directly and calls
  `note.body()` for content filters — no `Note::parse(path, text)` re-parse and no `needs_content` load fork.
- Replace the `loaded_notes` input to `SearchQuery` with `cached_notes` carrying the snapshot semantics
  above: when a snapshot is provided, search iterates it in-memory only; otherwise it walks the disk. The
  old "overlay that always re-walks disk for uncached notes" behavior is dropped — a provided snapshot is
  complete by construction.
- Drop the `loaded_notes` parameter from the free functions `find_all_tags`, `find_tags`,
  `find_notes_filtered`, and `find_notes_filtered_with_content`, and collapse the `_with_content` variants
  now that content is always loaded (`notes()` == `notes_with_content()`, etc.).

### LSP state layer (out of core)

The LSP is the only consumer of in-memory overrides, and only ever against a cached vault. The pre-indexing
window (`new_unindexed`, plain `Vault::open`) is where a `didOpen` could arrive before the snapshot exists.
Because `load_note()` now requires snapshot mode, **the LSP state layer is responsible for buffering open
documents until the cached vault is ready**, then applying them via `load_note()` once indexing completes.
Core does not need to support overriding notes in disk mode.

## Migration surface

Mechanical but broad; budget for a large diff:

- `pub body: Option<String>` → `text: String` + `body()` accessor touches direct `note.body` access in
  `vault.rs` (merge, rename_tag), CLI `note.rs` (`from_path_with_body` ×3), MCP `server.rs`
  (`load_body`), the LSP `notes_with_content()` helper and tests, and numerous core tests that assert on
  `note.body` or use `parse(..., keep_body)`.
- Update `CHANGELOG.md`, `README.md`, and `AGENTS.md` to reflect the new `Note`/`Vault` surface once the
  API stabilizes.
