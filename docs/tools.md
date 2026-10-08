---
title: Tool reference
description: All 41 MCP tools obsidian-brain exposes — arguments, behaviour, examples.
---

# Tool reference

41 tools, grouped by intent. Every tool description below includes a one-line Claude prompt you can copy-paste into chat to nudge routing in the right direction.

Every tool except `list_vaults` takes a required `vault` argument: one of the names given to `server --vault <name>=<path>`. There is no default vault.

## Vaults

### `list_vaults`

List the vaults this server serves, with each vault's path, note count and index state (`embedderReady`, `reindexInProgress`, and `initError` when its startup failed).

<!-- GENERATED:tool:list_vaults -->
_No arguments._
<!-- /GENERATED:tool:list_vaults -->

> "Which vaults do you have access to?"

## Find

### `search`

Find notes by meaning (chunk-level semantic similarity) or by exact text (SQLite FTS5 with Porter stemming + BM25 `title:body = 5:1`). The default `hybrid` mode fuses both rankings via Reciprocal Rank Fusion.

<!-- GENERATED:tool:search -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `query` | string | Natural-language query or keyword phrase. |
| `mode` | `"hybrid"` \| `"semantic"` \| `"fulltext"`? | Default `hybrid`. Semantic-only queries chunk vectors; fulltext-only queries FTS5. |
| `limit` | number? | Max results to return. Default 20. |
| `unique` | `"notes"` \| `"chunks"`? | Default `"notes"` (one row per note). Set `"chunks"` for raw chunk rows with chunkHeading, chunkStartLine, chunkExcerpt. |
<!-- /GENERATED:tool:search -->

The response is wrapped as `{data, context}` — `context.next_actions` suggests the most useful follow-up call (e.g. `read_note(top hit)`, `find_connections(top-3)`, or a simplified query retry on zero hits). Clients that ignore `context` keep working.

`mode: 'hybrid' + unique: 'chunks'` returns chunk metadata (including `chunkHeading`, `chunkStartLine`, `chunkExcerpt`). FTS5 queries containing `-`, `:`, `/`, or parens are auto-phrase-quoted — a query like `foo-bar-baz` no longer crashes.

> *"Use `search` to find notes semantically about supply-chain tax."*

### `list_notes`

List notes, optionally filtered by directory, tag, or link-target status. The tag filter reads frontmatter and inline tags, and a parent tag matches its nested tags (`tag: 'project'` matches `project/alpha`). Results are in path order; `sortBy: 'mtime'` puts the most recently modified notes first.

<!-- GENERATED:tool:list_notes -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `directory` | string? | Restrict to notes under this subdirectory prefix. |
| `tag` | string? | Restrict to notes with this tag or a tag nested below it. |
| `sortBy` | `"path"` \| `"mtime"`? | Default `path`. `mtime` sorts newest first and adds `mtime` to each result. |
| `limit` | number? | Max results to return. Default 100. |
| `includeStubs` | boolean? | Default `true`. Set `false` to exclude unresolved wiki-link targets. |
<!-- /GENERATED:tool:list_notes -->

> *"Use `list_notes` to list every note under `Projects/` tagged `#active`."*

### `read_note`

Read a note's metadata (and optionally its full body). Fuzzy-matches filenames, so "Q4 planning" resolves to `Meetings/2025-Q4 planning.md` if unambiguous.

<!-- GENERATED:tool:read_note -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string | Path, filename, or fuzzy match for the note to read. |
| `mode` | `"brief"` \| `"full"`? | Default `"brief"` (metadata + linked-note titles). `"full"` adds the body + edge context. |
| `maxContentLength` | number? | In `full` mode, max body chars before truncation. Default 2000. |
<!-- /GENERATED:tool:read_note -->

In `full` mode, the response includes `truncated: true` when the body exceeded `maxContentLength` and was sliced. Wrapped as `{data, context}` with `next_actions` hints — e.g. `create_note` for unresolved `[[links]]`, `find_connections` for outgoing neighbours.

> *"Use `read_note` to open the note called 'Q4 planning' with `mode: 'full'`."*

### `find_notes_by_name`

Find notes by a half-remembered name, the way Obsidian's quick switcher does. The query is matched case-insensitively against each note's filename, title and frontmatter `aliases`; typos and non-contiguous characters still match. Each hit reports `path`, `title`, `matchedOn` (`name`, `title` or `alias`), the matching `alias` when relevant and a `score` from 0 to 1, best first. Unresolved link targets are never returned.

<!-- GENERATED:tool:find_notes_by_name -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `query` | string | Name fragment. Typos and non-contiguous characters still match. |
| `folder` | string? | Only notes under this vault-relative folder. |
| `limit` | number? | Max results (1-100). Default 20. |
<!-- /GENERATED:tool:find_notes_by_name -->

> *"Use `find_notes_by_name` to find the note I called something like 'kelly crit'."*

### `grep_vault`

Search the text of the note files on disk, line by line like grep, with a literal string or a JavaScript regex. Frontmatter is searched too, and line numbers are the real 1-based lines of the file. Each file lists its matching lines with `before` and `after` context lines, and `moreMatches: true` when it had more than `maxMatchesPerFile`. Regexes longer than 500 characters or with a quantified group that holds a quantifier, like `(a+)+`, are refused. A scan stops after 2 seconds or at `limit` matching files and then reports `truncated: true` with `stoppedBy`.

<!-- GENERATED:tool:grep_vault -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `query` | string | Literal text, or a regex source without slashes when `regex` is true. |
| `regex` | boolean? | Treat `query` as a JavaScript regex. Default false. |
| `caseSensitive` | boolean? | Default false. |
| `folder` | string? | Only notes under this vault-relative folder. |
| `contextLines` | number? | Lines of context before and after each match. Default 1. |
| `limit` | number? | Max files with matches to return. Default 20. |
| `maxMatchesPerFile` | number? | Max matching lines per file. Default 5. |
<!-- /GENERATED:tool:grep_vault -->

> *"Use `grep_vault` to find every line under `Projects/` that mentions 'TODO(marts)'."*

### `query_notes`

Filter notes by their metadata with a [JsonLogic](https://jsonlogic.com) expression: the headless replacement for Dataview queries. The filter sees one record per note: `{path, folder, name, title, tags, frontmatter, mtime, size}`. `tags` merges frontmatter and inline tags without `#`, `frontmatter` includes Dataview-style `key:: value` inline fields, and `mtime` is an ISO 8601 string. The extra operator `has_tag` matches a tag case-insensitively and includes nested tags, so `area` matches `area/work`. Results are sorted by `path` unless `sort` names `title`, `mtime` or `frontmatter.<key>`; notes without the sort value come last.

<!-- GENERATED:tool:query_notes -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `filter` | object | JsonLogic expression; a note matches when it is truthy. `{"==": [1, 1]}` matches every note. |
| `fields` | array? | Frontmatter keys to return. Default all. |
| `sort` | object? | Default: by path, ascending. |
| `limit` | number? | Max results (1-500). Default 50. |
<!-- /GENERATED:tool:query_notes -->

> *"Use `query_notes` to list every note tagged #book with a rating of 4 or more, newest first."*

## Files

These tools work on vault files directly: reading notes in batches or in parts, inspecting any path, managing folders and attachments.

### `read_notes`

Read up to 20 notes in one call. Each name resolves on its own, so a missing or ambiguous name returns `{name, error}` in its slot and the rest of the batch still succeeds. Bodies are truncated at `maxContentLength` (default 2000 chars) per note, with `truncated: true` on the ones that were cut.

<!-- GENERATED:tool:read_notes -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `names` | array | Paths, filenames, or fuzzy matches of the notes to read. |
| `maxContentLength` | number? | Max body chars per note before truncation. Default 2000. |
<!-- /GENERATED:tool:read_notes -->

> *"Use `read_notes` to read `Widgets`, `Gadgets` and `Inbox/today` together."*

### `read_note_part`

Read part of a note from disk instead of the whole body. `outline` lists the headings with level and line; `heading` returns the section under a heading, down to the next heading of the same or a higher level (a nested path such as `Project > Notes` picks one of several equal headings); `lines` returns a line range; `block` returns the paragraph or list item that carries a `^blockId`. Line numbers are 1-based and count the frontmatter. Headings and block ids inside fenced code blocks are ignored.

<!-- GENERATED:tool:read_note_part -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string | Path, filename, or fuzzy match for the note. |
| `mode` | `"outline"` \| `"heading"` \| `"lines"` \| `"block"` | Which part to read. |
| `heading` | string? | For `heading`: heading text, or a nested path like `Parent > Child`. |
| `startLine` | number? | For `lines`: first line, 1-based, inclusive. |
| `endLine` | number? | For `lines`: last line, inclusive. Default and maximum: the end of the note. |
| `blockId` | string? | For `block`: the block id, with or without the leading `^`. |
<!-- /GENERATED:tool:read_note_part -->

> *"Use `read_note_part` to get the outline of `Projects/Plan`, then read only its `Goals` section."*

### `file_info`

Describe one vault path without its body: `kind` (`note`, `attachment` or `folder`), size in bytes, and `mtime`/`ctime` as ISO timestamps. For a note it adds outgoing, incoming and unresolved link counts from the index, plus heading count, open and done tasks, and the tag list from the file on disk. A folder reports its number of direct children.

<!-- GENERATED:tool:file_info -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `path` | string | Vault-relative path of a note, attachment or folder. |
<!-- /GENERATED:tool:file_info -->

> *"Use `file_info` on `Projects/Plan.md` to see how many open tasks and backlinks it has."*

### `create_folder`

Create a folder and any missing parents. Calling it on an existing folder succeeds with `created: false`; a file at the path is an error.

<!-- GENERATED:tool:create_folder -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `path` | string | Vault-relative folder path, e.g. `Projects/2026`. |
<!-- /GENERATED:tool:create_folder -->

> *"Use `create_folder` to make `Projects/2026/Q1`."*

### `delete_folder`

Permanently delete a folder. A non-empty folder is refused unless `recursive: true`. Every note inside leaves the index in the same call (node, edges, embedding, orphaned stubs), as with `delete_note`. The vault root and `.obsidian` are always refused. Requires `confirm: true`; `dryRun: true` reports file, folder and note counts with a sample of up to 50 paths.

<!-- GENERATED:tool:delete_folder -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `path` | string | Vault-relative folder path. |
| `confirm` | true | Must literally be `true` to execute. Guards against accidental deletion. |
| `recursive` | boolean? | Delete the folder with everything in it. Default false: refuse a non-empty folder. |
| `dryRun` | boolean? | If true, report what would be deleted without removing anything. |
<!-- /GENERATED:tool:delete_folder -->

> *"Use `delete_folder` with `dryRun: true` on `Archive/2019`, then delete it recursively."*

### `list_attachments`

List the non-markdown files in the vault or one folder, each with its size and `references`: the number of notes that embed or link to it through `![[file]]`, `[[file]]`, `![](path)` or `[](path)`. Targets match by vault-relative path, by path relative to the note, or by bare filename, as Obsidian resolves them. `unreferencedOnly: true` finds orphaned attachments.

<!-- GENERATED:tool:list_attachments -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `folder` | string? | Vault-relative folder to list, recursively. Default: the whole vault. |
| `extensions` | array? | Keep only these extensions, case-insensitive, e.g. `["png", ".pdf"]`. |
| `unreferencedOnly` | boolean? | Return only attachments no note references. |
| `limit` | number? | Max results. Default 100. |
| `offset` | number? | Results to skip, for paging. Default 0. |
<!-- /GENERATED:tool:list_attachments -->

> *"Use `list_attachments` with `unreferencedOnly: true` to find images no note uses any more."*

### `create_attachment`

Create a binary file from base64 content, with any missing parent folders. The decoded size is capped at 10 MB. An existing file is replaced only with `overwrite: true`. Markdown paths are refused: use `create_note` for notes.

<!-- GENERATED:tool:create_attachment -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `path` | string | Vault-relative file path with extension, e.g. `assets/diagram.png`. |
| `content` | string | File bytes, base64-encoded. |
| `overwrite` | boolean? | Replace an existing file. Default false. |
<!-- /GENERATED:tool:create_attachment -->

> *"Use `create_attachment` to save this PNG as `assets/diagram.png`."*

## Write

### `create_note`

Create a new note with frontmatter and auto-index it. `title:` is auto-injected from the filename unless you explicitly pass `frontmatter: { title: null }`.

<!-- GENERATED:tool:create_note -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `title` | string | Note title. Used as the filename base and auto-injected into frontmatter. |
| `content` | string | Markdown body (do not include frontmatter here). |
| `directory` | string? | Vault-relative subdirectory to create the note in. |
| `frontmatter` | object? | YAML frontmatter key/value map. `title` is auto-injected unless explicitly set. |
<!-- /GENERATED:tool:create_note -->

Creating a note that matches an existing `[[ForwardRef]]` stub automatically repoints the stub's inbound edges to the real note and deletes the stub.

> *"Use `create_note` to create `Meetings/2026-04-21 standup.md` with tags `[meeting, standup]`."*

### `create_note_from_template`

Create a note from an Obsidian core Templates template. The template is looked up by name in the templates folder set in `.obsidian/templates.json` (default `Templates/`), or by vault-relative path. `{{title}}`, `{{date}}`, `{{time}}` and `{{date:FORMAT}}` / `{{time:FORMAT}}` are filled with moment-style tokens, defaulting to the `dateFormat` / `timeFormat` in that settings file (`YYYY-MM-DD`, `HH:mm`). `variables` fills other `{{key}}` placeholders. The note is written and indexed the same way as `create_note`, and the call fails if the note exists. Templater `<% %>` code is not run: it stays in the note as written, and the result says so. Placeholders nothing filled are listed in `unresolved`.

<!-- GENERATED:tool:create_note_from_template -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `template` | string | Template name in the templates folder, or a vault-relative path. `.md` is optional. |
| `title` | string | Note title. Used as the filename base and for `{{title}}`. |
| `directory` | string? | Vault-relative subdirectory to create the note in. |
| `variables` | object? | Extra `{{key}}` substitutions. |
| `date` | string? | ISO date or date-time to use instead of now. |
<!-- /GENERATED:tool:create_note_from_template -->

> *"Use `create_note_from_template` with the `Meeting` template to create `Meetings/Kickoff` with `attendees: 'Ann, Bo'`."*

### `edit_note`

Modify an existing note. Six modes: `append`, `prepend`, `replace_window` (find-and-replace; optionally fuzzy), `patch_heading`, `patch_frontmatter`, `at_line`.

<!-- GENERATED:tool:edit_note manual -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string | Path or fuzzy match. |
| `mode` | one of the six | Required. |
| `content` | string | New content (mode-dependent). |
| `search` | string | For `replace_window`: the block of text to locate. |
| `fuzzy` | boolean | For `replace_window`: tolerate whitespace + trailing punctuation drift. |
| `heading` | string | Target heading (for `patch_heading`). |
| `headingOp` | `"replace"` \| `"before"` \| `"after"` | For `patch_heading`. `replace` (default) replaces the section below the heading; `before` / `after` insert adjacent to the heading line. |
| `scope` | `"section"` \| `"body"` | For `patch_heading replace`: `section` (default) consumes until the next same-or-higher heading or EOF; `body` stops at the first blank line. |
| `headingIndex` | number | For `patch_heading` when the heading text appears more than once — 0-indexed top-to-bottom picker. Without it, multiple matches throw `MultipleMatchesError` listing each occurrence with line numbers. |
| `line` / `lineOp` | | For `at_line`. |
| `key` / `value` / `valueJson` | | For `patch_frontmatter`. Use `valueJson` from clients that stringify tool params (e.g. `valueJson: 'null'` to clear a key, `valueJson: 'true'` for a real boolean, `valueJson: '42'` for a number). |
| `expectedContent` | string? | Guard for edits that replace text (`replace_window`, `patch_heading` with `headingOp: replace`, `at_line` with `lineOp: replace`): the text being replaced, as last read. If the note changed since, the edit fails, nothing is written, and the error shows the current text. Line endings and trailing whitespace are ignored. Not with `edits`. |
<!-- /GENERATED:tool:edit_note -->

`patch_heading` responses include `removedLen` so callers can detect greedy trailing-heading consumption.

- `dryRun: true` → returns a unified diff + `previewId`; no file is mutated. Commit the preview with `apply_edit_preview({ previewId })`.
- `edits: [...]` — bulk edit array applied atomically on a single file. All or nothing; error names the failing index if any edit fails.
- `fuzzyThreshold: 0–1` on `replace_window` (default `0.7`). Higher = stricter match required.
- `from_buffer: true` — on `replace_window` NoMatch, the proposed content is held in a buffer; retry via `from_buffer: true` retries with `fuzzy: true, fuzzyThreshold: 0.5`.

> *"Use `edit_note` to append a 'Follow-ups' section to today's standup note."*

### `apply_edit_preview`

Commit an edit previewed via `edit_note({ dryRun: true })`.

```
apply_edit_preview({ previewId: "prev_..." })
```

<!-- GENERATED:tool:apply_edit_preview -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `previewId` | string | The previewId returned by `edit_note` with `dryRun: true`. |
<!-- /GENERATED:tool:apply_edit_preview -->

- Preview not found or expired (5 min TTL) → error; regenerate the preview.
- Target file changed since preview was generated → error; regenerate the preview.

### `link_notes`

Add a wiki-link between two notes plus a "why this connects" context sentence placed where the link is inserted.

<!-- GENERATED:tool:link_notes -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `source` | string | Source note to add the link from (path or fuzzy match). |
| `target` | string | Target note to link to (path, title, or new wiki-link ref). |
| `context` | string | One-sentence explanation of why these notes are connected. |
| `dryRun` | boolean? | If true, return the line that would be appended without writing. |
<!-- /GENERATED:tool:link_notes -->

`dryRun: true` returns the line that would be appended without writing.

> *"Use `link_notes` to link `Bayesian updating` to `Kelly criterion` with a note about risk-adjusted bets."*

### `move_note`

Rename or move a note. All inbound wiki-links (`[[old]]`, `[[old|alias]]`, `![[old]]`, `[[old#heading]]`, `[[old^block]]`) are rewritten in place across every note that linked to the old stem; graph edges stay intact.

<!-- GENERATED:tool:move_note -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `source` | string | Current path or fuzzy match of the note to move. |
| `destination` | string | New vault-relative path (including `.md`). `.md` is appended automatically if omitted. |
| `dryRun` | boolean? | If true, report what would be rewritten without mutating any files. |
<!-- /GENERATED:tool:move_note -->

Response adds `linksRewritten: {files, occurrences}` counting the rewrites applied.

`dryRun: true` reports what would be rewritten without mutating. Response on a real move includes `stubsPruned: N`.

> *"Use `move_note` with `source: 'Inbox/thought.md'` and `destination: 'Areas/Ideas/thought.md'`."*

### `delete_note`

Delete a note. Requires `confirm: true` as a Zod-level guard.

<!-- GENERATED:tool:delete_note -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string | Path or fuzzy match of the note to delete. |
| `confirm` | true | Must literally be `true` to execute. Guards against accidental deletion. |
| `dryRun` | boolean? | If true, report what would be deleted without removing any files. |
<!-- /GENERATED:tool:delete_note -->

When the delete removed inbound edges, the response is wrapped in a `{data, context: {next_actions}}` envelope suggesting `rank_notes({metric: 'influence', minIncomingLinks: 0})` as a follow-up to surface newly orphaned notes.

`dryRun: true` reports what would be deleted. Real deletes surface `deletedFromIndex.stubsPruned: N` when the deleted note's orphan-stub targets were cleaned up.

> *"Use `delete_note` with `confirm: true` to delete `Inbox/obsolete.md`."*

## Properties

### `list_property_values`

The distinct values of one frontmatter property across notes, with how often each occurs, ordered by count. Each element of a list value counts separately, and a number stays distinct from the same text as a string. `notesWithKey` tells how many notes have the key at all.

<!-- GENERATED:tool:list_property_values -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `key` | string | Frontmatter key, e.g. `status`. |
| `folder` | string? | Only notes under this folder. |
| `limit` | number? | Max distinct values to return. Default 100. |
<!-- /GENERATED:tool:list_property_values -->

> *"Use `list_property_values` for `status` under `Projects/` to see which statuses I use."*

### `update_properties`

Set and remove several frontmatter properties of one note in a single atomic write. Creates the frontmatter block when the note has none and drops it when the last key goes. The body and keys not named stay as they are; the YAML is re-serialised the same way as `edit_note`'s `patch_frontmatter`. A key named in both `set` and `remove` is set. `dryRun: true` returns the new frontmatter without writing.

<!-- GENERATED:tool:update_properties -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string | Path or fuzzy match of the note. |
| `set` | object? | Keys to write, with their values. A `null` value removes the key. |
| `remove` | array? | Keys to remove. Absent keys are skipped. |
| `dryRun` | boolean? | Return the new frontmatter without writing. |
<!-- /GENERATED:tool:update_properties -->

> *"Use `update_properties` on 'Q4 planning' to set `status: done` and remove `due`."*

## Structure

### `vault_overview`

One-call orientation for a vault: note count, attachment count (non-Markdown files outside hidden folders), notes per top-level folder (`(root)` for notes at the top), the most used tags with note counts, and the most recently modified notes with their index mtime. Use `list_tags` or `list_notes` with `sortBy: "mtime"` for more than the snapshot.

<!-- GENERATED:tool:vault_overview -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `topTags` | number? | Tags to include. Default 15. |
| `recent` | number? | Recently modified notes to include. Default 10. |
<!-- /GENERATED:tool:vault_overview -->

> *"Use `vault_overview` to get a quick picture of my vault before we start."*

### `list_tags`

Every tag with the number of notes that carry it. Reads frontmatter `tags` and `tag` (a list or a comma-separated string, with or without `#`) and inline `#tags` in the body. By default a note tagged `a/b` also counts toward `a`; pass `includeParents: false` to count each nested tag only under its own name.

<!-- GENERATED:tool:list_tags -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `sort` | `"count"` \| `"name"`? | Default `count` (descending). `name` sorts alphabetically. |
| `limit` | number? | Max tags to return. Default 100. |
| `prefix` | string? | Only tags starting with this text, e.g. `project/`. |
| `includeParents` | boolean? | Default `true`: a note tagged `a/b` also counts toward `a`. |
<!-- /GENERATED:tool:list_tags -->

> *"Use `list_tags` with `prefix: 'project/'` to show which project tags I use most."*

### `list_bookmarks`

The bookmarks of the Obsidian core Bookmarks plugin, read from `.obsidian/bookmarks.json`, as a tree of groups, files, folders, searches, headings and blocks. Returns an empty list with a `note` when the file is absent or not valid. A `types` filter without `group` flattens group contents to the top level.

<!-- GENERATED:tool:list_bookmarks -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `types` | array? | Only these bookmark types. Without `group`, group contents are flattened. |
<!-- /GENERATED:tool:list_bookmarks -->

> *"Use `list_bookmarks` to show the notes I bookmarked."*

## Tasks

### `list_tasks`

List markdown tasks (`- [ ] text`, `* [x] text`, `1. [/] text`) across the vault, one folder, or one note, read from the files on disk. Tasks inside fenced code and frontmatter are skipped. Each task has its 1-based `line`, the raw `status` character, a `state` (`x`/`X` is done, `-` is cancelled, every other character is open), the `text`, its nesting `indent`, `parentLine` when it sits under another task, and `due` from an Obsidian Tasks `📅 YYYY-MM-DD` date. `total` and `truncated` support paging with `limit` and `offset`.

<!-- GENERATED:tool:list_tasks -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string? | Path or fuzzy match of one note. Omit to scan the vault. |
| `folder` | string? | Vault-relative folder to scan recursively. |
| `status` | `"open"` \| `"done"` \| `"all"`? | Default `open`: every status but `x`, `X` and `-`. `done`: `x`/`X`. `all` adds cancelled (`-`). |
| `limit` | number? | Max tasks to return. Default 200, max 1000. |
| `offset` | number? | Tasks to skip, for paging. Default 0. |
<!-- /GENERATED:tool:list_tasks -->

> *"Use `list_tasks` to show my open tasks under `Projects/`."*

### `set_task_status`

Set the status of one task in place. Only the character between the brackets changes; indentation, list marker, text and line endings stay byte-identical. Pass `expectedText` (the `text` from `list_tasks`) to refuse the write when the line no longer holds that task. A line that is not a task is refused.

<!-- GENERATED:tool:set_task_status -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string | Path or fuzzy match of the note. |
| `line` | number | 1-based line of the task, as `list_tasks` returns it. |
| `status` | string | `open` (space), `done` (`x`), or one raw status character such as `/` or `-`. |
| `expectedText` | string? | The task text you expect on that line. The write is refused when it differs, e.g. after the note changed. |
<!-- /GENERATED:tool:set_task_status -->

> *"Use `set_task_status` to mark the 'Write spec' task in `Projects/Launch` as done."*

## Blocks

### `ensure_block_id`

Return the `^block-id` of a block, adding one when the block has none, with a ready `[[Note#^id]]` link. Target the block by any line inside it, or by a heading, in which case the heading line carries the id. A paragraph, list item or heading gets ` ^id` at the end of its line; a table, quote, callout or fenced code block gets `^id` on its own line after the block, set off by blank lines. A block that already has an id keeps it. `dryRun: true` returns the id and diff without writing.

<!-- GENERATED:tool:ensure_block_id -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string | Path or fuzzy match of the note. |
| `line` | number? | 1-based line inside the block. |
| `heading` | string? | Heading text; the id goes on that heading line. |
| `id` | string? | Id to write when the block has none: letters, digits, dashes. Default: random 6 characters. |
| `dryRun` | boolean? | If true, return the id and diff without writing. |
<!-- /GENERATED:tool:ensure_block_id -->

> *"Use `ensure_block_id` on line 12 of `Meetings/Kickoff` and give me a link to that paragraph."*

## Canvas

### `read_canvas`

Read a `.canvas` file (JSON Canvas 1.0) and return its nodes and edges as stored, including fields this server does not model. An empty file reads as an empty canvas.

<!-- GENERATED:tool:read_canvas -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `path` | string | Vault-relative path of the `.canvas` file. The extension is optional. |
<!-- /GENERATED:tool:read_canvas -->

> *"Use `read_canvas` to show me what is on `Projects/Roadmap.canvas`."*

### `edit_canvas`

Make one change to a canvas per call. `add_node` adds a `text`, `file`, `link` or `group` node; without `x`/`y` it goes right of the right-most node, at 400×200 unless sized, and a missing canvas is created. `update_node` changes only the fields passed, and `null` removes `color`, `label` or `subpath`. `delete_node` also removes the node's edges. `connect` adds an edge between two existing nodes, and `disconnect` removes one by id. Ids are 16 hex characters, as Obsidian makes them. Every other field of existing nodes and edges is kept. A `file` node whose target is missing gets a warning, not an error. The file is written atomically in Obsidian's layout: tab-indented, one node or edge per line.

<!-- GENERATED:tool:edit_canvas -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `path` | string | Vault-relative path of the `.canvas` file. The extension is optional. |
| `operation` | `"add_node"` \| `"update_node"` \| `"delete_node"` \| `"connect"` \| `"disconnect"` |  |
| `nodeId` | string? | Node to change (`update_node`, `delete_node`). |
| `type` | `"text"` \| `"file"` \| `"link"` \| `"group"`? | Node type (`add_node`). |
| `text` | string? | Markdown content of a `text` node. |
| `file` | string? | Vault-relative file a `file` node shows. |
| `subpath` | string \| null? | Heading or block of a `file` node, e.g. `#Heading`. `null` removes it. |
| `url` | string? | URL of a `link` node. |
| `label` | string \| null? | Label of a `group` node or an edge. `null` removes it. |
| `color` | string \| null? | Preset `"1"`-`"6"` or hex like `"#ff0000"`. `null` removes it. |
| `x` | number? |  |
| `y` | number? |  |
| `width` | number? | Default 400. |
| `height` | number? | Default 200. |
| `fromNode` | string? | Edge start node id (`connect`). |
| `toNode` | string? | Edge end node id (`connect`). |
| `fromSide` | `"top"` \| `"right"` \| `"bottom"` \| `"left"`? |  |
| `toSide` | `"top"` \| `"right"` \| `"bottom"` \| `"left"`? |  |
| `fromEnd` | `"none"` \| `"arrow"`? | Default `none`. |
| `toEnd` | `"none"` \| `"arrow"`? | Default `arrow`. |
| `edgeId` | string? | Edge to remove (`disconnect`). |
<!-- /GENERATED:tool:edit_canvas -->

> *"Use `edit_canvas` to add a text node 'Open questions' to `Projects/Roadmap.canvas` and connect it to the `Launch` node."*

## Map the graph

### `find_connections`

N-hop link neighborhood around a note. Returns inbound + outbound links grouped by hop distance, optionally the full subgraph for visualization.

<!-- GENERATED:tool:find_connections -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string | Starting note (path or fuzzy match). |
| `depth` | number? | Number of hops to traverse. Default 1, max 3. |
| `returnSubgraph` | boolean? | Return all edges in the neighborhood as a full subgraph instead of a flat list. |
| `includeStubs` | boolean? | Default `false`. Set `true` to include broken-wikilink stub neighbours (`frontmatter._stub: true`). |
<!-- /GENERATED:tool:find_connections -->

Response is wrapped as `{data, context}` — `context.next_actions` suggests `find_path_between` to the furthest neighbour. Clients that ignore `context` keep working.

#### `find_connections` `context` envelope shape

```jsonc
{
  "data": [ /* neighbours, or {nodes, edges} when returnSubgraph: true */ ],
  "context": {
    "state": {
      "last_connections_root": "Epistemology.md",
      "last_connections_count": 7
    },
    "next_actions": [
      { "description": "Trace path from Epistemology.md to <furthest>.md via find_path_between" }
    ]
  }
}
```

- `context.state.last_connections_root` — the resolved-from path of the call (lets the client correlate follow-ups).
- `context.state.last_connections_count` — neighbour count returned this call.
- `context.next_actions[]` — list of single-sentence suggestions an LLM can route directly into the next tool call. Empty when no useful follow-up exists (zero neighbours, depth=1 on a single-edge note). Skipping `context` is always safe — `data` carries everything load-bearing.

> *"Use `find_connections` to show everything within 2 hops of `Epistemology.md`."*

### `find_path_between`

Shortest link chain(s) between two notes. Optionally return their shared neighbors as well.

<!-- GENERATED:tool:find_path_between -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `from` | string | Source note (path or fuzzy match). |
| `to` | string | Target note (path or fuzzy match). |
| `maxDepth` | number? | Maximum path length in hops. Default 3. |
| `includeCommon` | boolean? | Also return notes that both `from` and `to` link to (shared neighbors). |
| `includeStubs` | boolean? | Default `false`. Set `true` to include broken-wikilink stub nodes (`frontmatter._stub: true`) in the path search. |
<!-- /GENERATED:tool:find_path_between -->

> *"Use `find_path_between` to find how `Bayesian updating` connects to `Kelly criterion`."*

### `rank_notes`

Top notes by `influence` (PageRank over backlinks), `bridging` (betweenness centrality, normalized 0–1 so scores compare across vaults), or `both`.

<!-- GENERATED:tool:rank_notes -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `metric` | `"influence"` \| `"bridging"` \| `"both"`? | Ranking metric. Default `"both"`. `"influence"` = PageRank; `"bridging"` = betweenness centrality. |
| `limit` | number? | Max results to return. Default 20. |
| `includeStubs` | boolean? = false | Default `false`. Set `true` to include unresolved wiki-link target stubs (`frontmatter._stub: true`) in the ranked set. With stubs in, popular link targets dominate eigenvector-style centrality even when they have no real content behind them. |
| `minIncomingLinks` | number? = 2 | Minimum incoming links for influence ranking. Default 2. Pass 0 to see unfiltered PageRank. |
<!-- /GENERATED:tool:rank_notes -->

> *"Use `rank_notes` with `metric: 'influence'` to list the top 10 most-linked-to notes."*

## Maintenance

### `reindex`

Force a full re-index. You rarely need this — the live watcher picks up file changes automatically. Fall back to `reindex` if your vault lives somewhere FSEvents/inotify can't observe (SMB, NFS), or after bulk edits outside Claude. It also prunes orphan stubs.

<!-- GENERATED:tool:reindex -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
<!-- /GENERATED:tool:reindex -->

Response includes `stubsPruned: N` — the one-shot migration path for users upgrading from older versions with pre-fix orphan stubs.

#### `reindex` response field semantics

The `reindex` response carries several `*Created` / `*Pruned` / `*Indexed` counters. They're all **deltas for this run**, not totals:

- `nodesIndexed` — notes whose content was re-embedded this run.
- `nodesSkipped` — notes whose mtime hasn't changed since the last index pass (skipped via the `sync` table).
- `edgesIndexed` — wiki-link edges materialised this run.
- `stubNodesCreated` — broken-wikilink target stubs (`_stub/Foo.md`) **newly materialised this run**, not the total stub count in the graph. A vault with 2,433 long-standing stubs and zero new ones reports `stubNodesCreated: 0`.
- `stubsPruned` — orphan stub nodes **deleted this run** (stubs whose only inbound edges came from a note that was just deleted, or stubs that got promoted to real notes because a matching `.md` file appeared).

> *"Use `reindex` to refresh the index after I bulk-edited files outside Claude."*

### `index_status`

Read-only inspection of index health. Surfaces everything an LLM client needs to answer "is semantic search working?" without mutating any state.

<!-- GENERATED:tool:index_status -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
<!-- /GENERATED:tool:index_status -->

Response fields:

- `embeddingModel`, `embeddingDim`, `provider` — active embedder identity.
- `notesTotal`, `notesWithEmbeddings`, `notesNoEmbeddableContent`, `notesMissingEmbeddings`, `chunksTotal` — coverage counts. `notesNoEmbeddableContent` covers notes recorded with reason `'no-embeddable-content'` in `failed_chunks` (empty / frontmatter-only / sub-`minChunkChars` body), so `notesMissingEmbeddings` reflects only genuine failures, not the daily-note tail.
- `summary` — one-line human-readable string ("X / Y notes indexed; Z have no embeddable content; W failed to embed"), so MCP clients can report status without conflating buckets.
- `chunksSkippedInLastRun`, `failedChunks[]`, `failedChunksTotal` — fault-tolerant skip-log (`failed_chunks` table). Each `failedChunks` entry has `note`, `reason` (one of `too-long` / `embed-error` / `note-too-long` / `note-embed-error` / `no-embeddable-content`), `at` (epoch ms).
- `advertisedMaxTokens`, `discoveredMaxTokens` — capacity bounds (`embedder_capability` table). `advertised` from the bundled seed / metadata-cache or from the tokenizer / `/api/show`; `discovered` shrinks if embed failures reveal a tighter ceiling, floored at `MIN_DISCOVERED_TOKENS=256` and reset to `advertised` at the start of every full reindex.
- `prefixSource` — provenance of the active query/document prefixes. One of `'override'` (user set via `models override` / `models add`), `'seed'` (came from `data/seed-models.json`), `'metadata'` (live HF `config_sentence_transformers.json`), `'metadata-base'` (upstream `base_model`'s same JSON), `'readme'` (Tier 3 README fingerprinting), `'fallback'` (HF unreachable; safe defaults), `'none'` (symmetric — no prefix needed).
- `overrideApplied` — boolean; true iff a user override at `~/.config/obsidian-brain/model-overrides.json` patched any of the resolved fields. Useful for diagnostics: distinguishes "the seed says X" from "the user overrode X to Y."
- `lastReindexReasons` — the bootstrap's stated reasons for any reindex on last boot (e.g. model switch, prefix-strategy version bump, schema migration).
- `reindexInProgress` — boolean; true while a background reindex is running.
- `embedderReady`, `initError` — live ctx state; any startup failure surfaces in `initError` as `"Name: message"`.

> *"Use `index_status` to check whether semantic search is ready and see how many chunks were skipped in the last reindex."*

### `find_broken_links`

Scan note bodies for `[[wikilinks]]` that do not resolve. A link is broken when its target note does not exist (`note_not_found`, the links the index stores as `_stub/` edges), or when the target exists but lacks the `#heading` (`heading_not_found`) or `#^block` id (`block_not_found`) the link names. Heading matches ignore case, as Obsidian does. Links inside code, embeds, and links to attachments that exist on disk are skipped. Each result gives the source path, the 1-based line in the file, and the raw link text. Read-only.

<!-- GENERATED:tool:find_broken_links -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `folder` | string? | Only scan notes under this folder. |
| `excludeFolders` | array? | Folders to skip. |
| `limit` | number? | Max broken links returned. Default 100. |
<!-- /GENERATED:tool:find_broken_links -->

> *"Use `find_broken_links` to list every link in my Projects folder that points at a missing note or heading."*

### `find_orphaned_notes`

List notes that no other note links to. Self-links do not count, and unresolved link targets (stubs) are not notes. `excludeFolders` hides notes from the result, but links from those folders still count as incoming links. Each result gives the path, the title, and the number of outgoing links, so you can tell a dead end from a hub nobody points at. Read-only.

<!-- GENERATED:tool:find_orphaned_notes -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `folder` | string? | Only report notes under this folder. |
| `excludeFolders` | array? | Folders whose notes are not reported. Their links still count. |
| `limit` | number? | Max orphans returned. Default 100. |
<!-- /GENERATED:tool:find_orphaned_notes -->

> *"Use `find_orphaned_notes` to show notes nothing links to, ignoring my templates folder."*

### `search_and_replace`

Find and replace text in note bodies across the vault or one folder. The pattern is plain text by default; with `regex: true` it is a JavaScript regex where `^`/`$` match at line ends and `$1`, `$<name>` and `$&` expand in the replacement. Frontmatter and fenced code blocks stay untouched unless `includeFrontmatter` or `includeCode` is set. Patterns over 500 characters and patterns with nested quantifiers are refused. `dryRun` defaults to `true` and returns per-file match counts with up to three before/after line samples. A write is refused when more than `maxFiles` files match; files are written atomically and the index refreshes once afterwards.

<!-- GENERATED:tool:search_and_replace -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `pattern` | string | Text to find, or a JavaScript regex when `regex` is true. `^`/`$` match at line ends. |
| `replacement` | string | Replacement text. With `regex`, `$1`, `$<name>` and `$&` expand. |
| `regex` | boolean? | Treat `pattern` as a regex. Default false. |
| `caseSensitive` | boolean? | Default true. |
| `folder` | string? | Only touch notes under this folder. |
| `includeFrontmatter` | boolean? | Also replace inside YAML frontmatter. Default false. |
| `includeCode` | boolean? | Also replace inside fenced code blocks. Default false. |
| `dryRun` | boolean? | Default true. Pass false to write. |
| `maxFiles` | number? | Refuse to write when more files match. Default 200. |
<!-- /GENERATED:tool:search_and_replace -->

> *"Use `search_and_replace` to preview replacing 'Acme Corp' with 'Acme Inc' everywhere, then apply it."*

### `rename_tag`

Rename a tag everywhere: inline `#tag` occurrences in note bodies and entries in the `tags` / `tag` frontmatter keys (lists, flow lists or strings, with or without `#`). Tags match case-insensitively and as whole tags, so `#old` never touches `#older`. With `includeNested` (default `true`), `#old/child` becomes `#new/child`. Tags in code, URL fragments and headings are left alone. `dryRun` defaults to `true` and returns per-file inline and frontmatter counts. Files are written atomically and the index refreshes once afterwards.

<!-- GENERATED:tool:rename_tag -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `from` | string | Tag to rename, `#` optional. Case-insensitive. |
| `to` | string | New tag name, `#` optional. |
| `includeNested` | boolean? | Also rename `#from/child` to `#to/child`. Default true. |
| `dryRun` | boolean? | Default true. Pass false to write. |
<!-- /GENERATED:tool:rename_tag -->

> *"Use `rename_tag` to rename #proj to #project, including its nested tags."*

### `rename_heading`

Rename one heading in a note and rewrite every link to it across the vault: `[[Note#Old]]`, `[[Note#Old|alias]]`, `![[Note#Old]]` embeds, `[[#Old]]` inside the note itself, and heading paths such as `[[Note#Parent#Old]]`. The tool fails when `from` is not a heading in the note, appears more than once, or when `to` already exists in the note. Returns the files changed and the number of links rewritten; `dryRun: true` reports the same without writing. Files are written atomically and the index refreshes once afterwards.

<!-- GENERATED:tool:rename_heading -->
| Arg | Type | Description |
|---|---|---|
| `vault` | string | The vault to work in. `list_vaults` describes each vault. |
| `name` | string | Path or fuzzy match of the note. |
| `from` | string | Current heading text, without `#`. |
| `to` | string | New heading text, without `#`. |
| `dryRun` | boolean? | If true, report the changes without writing. Default false. |
<!-- /GENERATED:tool:rename_heading -->

> *"Use `rename_heading` to rename the 'Notes' heading in my Weekly Review note to 'Reflections' and fix the links to it."*

---

## Capability matrix

| Tool | Works offline | Writes to vault |
|---|:-:|:-:|
| `search` | ✅ | — |
| `list_notes` | ✅ | — |
| `read_note` | ✅ | — |
| `find_notes_by_name` | ✅ | — |
| `grep_vault` | ✅ | — |
| `query_notes` | ✅ | — |
| `find_connections` | ✅ | — |
| `find_path_between` | ✅ | — |
| `rank_notes` | ✅ | — |
| `create_note` | ✅ | ✅ |
| `edit_note` | ✅ | ✅ |
| `apply_edit_preview` | ✅ | ✅ |
| `link_notes` | ✅ | ✅ |
| `move_note` | ✅ | ✅ |
| `create_note_from_template` | ✅ | ✅ |
| `delete_note` | ✅ | ✅ |
| `read_canvas` | ✅ | — |
| `edit_canvas` | ✅ | ✅ |
| `read_notes` | ✅ | — |
| `read_note_part` | ✅ | — |
| `file_info` | ✅ | — |
| `create_folder` | ✅ | ✅ |
| `delete_folder` | ✅ | ✅ |
| `list_attachments` | ✅ | — |
| `create_attachment` | ✅ | ✅ |
| `list_tasks` | ✅ | — |
| `set_task_status` | ✅ | ✅ |
| `ensure_block_id` | ✅ | ✅ |
| `vault_overview` | ✅ | — |
| `list_tags` | ✅ | — |
| `list_bookmarks` | ✅ | — |
| `list_property_values` | ✅ | — |
| `update_properties` | ✅ | ✅ |
| `reindex` | ✅ | — |
| `index_status` | ✅ | — |
| `find_broken_links` | ✅ | — |
| `find_orphaned_notes` | ✅ | — |
| `search_and_replace` | ✅ | ✅ |
| `rename_tag` | ✅ | ✅ |
| `rename_heading` | ✅ | ✅ |
