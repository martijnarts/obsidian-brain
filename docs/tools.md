# Tool reference

The server has 41 tools. This page lists each tool with the description and the arguments the server sends in `tools/list`. `tests/tool_schemas.rs` checks the server against the same interface.

Every tool except `list_vaults` takes a required `vault` argument: one of the names given to `server --vault <name>=<path>`.

## Vaults

### `list_vaults`

List the vaults this server serves. Every other tool takes one of these names as its required `vault` argument.

No arguments.

## Find

### `search`

Search vault notes. `hybrid` (default) fuses semantic + full-text ranks via Reciprocal Rank Fusion — no tuning needed, best for most queries. `semantic` is concept-only (better for abstract/paraphrased queries). `fulltext` is literal-token (better when you know the exact phrase exists). Semantic search is chunk-level — results are deduped to one-per-note by default. Set `unique: "chunks"` to return chunk-level hits with `chunkHeading`, `chunkStartLine`, and `chunkExcerpt`; supported by `semantic` and `hybrid` modes. `fulltext` is note-level only and ignores `unique` (full-text chunk search is not yet supported). Response is wrapped as `{data, context}` where `context.next_actions` suggests the agent's most useful follow-up call (read top hit, explore connections, or retry with broader phrasing on zero hits).

| Argument | Type | Required | Description |
|---|---|---|---|
| `query` | string | yes | Natural-language query or keyword phrase. |
| `mode` | `"hybrid"` \| `"semantic"` \| `"fulltext"` | no | Default `hybrid`. Semantic-only queries chunk vectors; fulltext-only queries FTS5. |
| `limit` | integer | no | Max results to return. Default 20. |
| `unique` | `"notes"` \| `"chunks"` | no | Default `"notes"` (one row per note). Set `"chunks"` for raw chunk rows with chunkHeading, chunkStartLine, chunkExcerpt. |

### `list_notes`

List notes in the vault. Optionally filter by directory prefix or by tag (frontmatter or inline; `a` also matches `a/b`). Pass `sortBy: "mtime"` for the most recently modified notes first. Pass `includeStubs: false` to exclude unresolved wiki-link targets (nodes with `frontmatter._stub: true`) and see only real on-disk notes.

| Argument | Type | Required | Description |
|---|---|---|---|
| `directory` | string | no | Restrict to notes under this subdirectory prefix. |
| `tag` | string | no | Restrict to notes with this tag or a tag nested below it. |
| `sortBy` | `"path"` \| `"mtime"` | no | Default `path`. `mtime` sorts newest first and adds `mtime` to each result. |
| `limit` | integer | no | Max results to return. Default 100. |
| `includeStubs` | boolean | no | Default `true`. Set `false` to exclude unresolved wiki-link targets. |

### `read_note`

Read a note's content. Brief mode (default) returns title + metadata + linked-note titles; full mode returns full content + edge context. Full mode also reports `truncated: true` when the body exceeded `maxContentLength` (default 2000 chars) and was sliced. Response is wrapped as `{data, context}` where `context.next_actions` suggests follow-ups like creating missing linked notes or exploring outgoing connections.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Path, filename, or fuzzy match for the note to read. |
| `mode` | `"brief"` \| `"full"` | no | Default `"brief"` (metadata + linked-note titles). `"full"` adds the body + edge context. |
| `maxContentLength` | integer | no | In `full` mode, max body chars before truncation. Default 2000. |

### `find_notes_by_name`

Find notes by a half-remembered name, like Obsidian's quick switcher: fuzzy, case-insensitive match against the filename, title and frontmatter `aliases`, best first. Use `grep_vault` or `search` to look inside note bodies instead.

| Argument | Type | Required | Description |
|---|---|---|---|
| `query` | string | yes | Name fragment. Typos and non-contiguous characters still match. |
| `folder` | string | no | Only notes under this vault-relative folder. |
| `limit` | integer | no | Max results (1-100). Default 20. |

### `grep_vault`

Search the text of every note file on disk, frontmatter included, for a literal string or a JavaScript regex, line by line like grep. Returns matching lines with 1-based line numbers and surrounding lines; `truncated: true` means the scan stopped early at the file limit or the 2 s time cap.

| Argument | Type | Required | Description |
|---|---|---|---|
| `query` | string | yes | Literal text, or a regex source without slashes when `regex` is true. |
| `regex` | boolean | no | Treat `query` as a JavaScript regex. Default false. |
| `caseSensitive` | boolean | no | Default false. |
| `folder` | string | no | Only notes under this vault-relative folder. |
| `contextLines` | integer | no | Lines of context before and after each match. Default 1. |
| `limit` | integer | no | Max files with matches to return. Default 20. |
| `maxMatchesPerFile` | integer | no | Max matching lines per file. Default 5. |

### `query_notes`

Filter notes by metadata with a JsonLogic `filter`; the headless replacement for Dataview queries. Each note is `{path, folder, name, title, tags, frontmatter, mtime, size}`: `tags` merges frontmatter and inline tags without `#`, `mtime` is ISO 8601, and `{"var": "frontmatter.status"}` reads a property. The extra operator `has_tag` matches a tag case-insensitively, nested tags included. Example: `{"and": [{"has_tag": [{"var": "tags"}, "book"]}, {">=": [{"var": "frontmatter.rating"}, 4]}]}`.

| Argument | Type | Required | Description |
|---|---|---|---|
| `filter` | object | yes | JsonLogic expression; a note matches when it is truthy. `{"==": [1, 1]}` matches every note. |
| `fields` | array of string | no | Frontmatter keys to return. Default all. |
| `sort` | object | no | Default: by path, ascending. |
| `limit` | integer | no | Max results (1-500). Default 50. |

## Files

### `read_notes`

Read up to 20 notes in one call. Returns one entry per name, in order: `{path, title, frontmatter, content, truncated}`, or `{name, error}` when that name does not resolve.

| Argument | Type | Required | Description |
|---|---|---|---|
| `names` | array of string | yes | Paths, filenames, or fuzzy matches of the notes to read. |
| `maxContentLength` | integer | no | Max body chars per note before truncation. Default 2000. |

### `read_note_part`

Read part of a note from disk: its heading `outline`, the section under a `heading`, a range of `lines`, or the `block` that carries a `^blockId`. Line numbers are 1-based and count the frontmatter.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Path, filename, or fuzzy match for the note. |
| `mode` | `"outline"` \| `"heading"` \| `"lines"` \| `"block"` | yes | Which part to read. |
| `heading` | string | no | For `heading`: heading text, or a nested path like `Parent > Child`. |
| `startLine` | integer | no | For `lines`: first line, 1-based, inclusive. |
| `endLine` | integer | no | For `lines`: last line, inclusive. Default and maximum: the end of the note. |
| `blockId` | string | no | For `block`: the block id, with or without the leading `^`. |

### `file_info`

Describe a vault path without its body: kind (note, attachment, folder), size, times, and for a note its link, heading, task and tag counts. Link counts come from the index.

| Argument | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Vault-relative path of a note, attachment or folder. |

### `create_folder`

Create a folder in the vault, with any missing parent folders. Succeeds with `created: false` when the folder already exists.

| Argument | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Vault-relative folder path, e.g. `Projects/2026`. |

### `delete_folder`

Permanently delete a folder. Refuses a non-empty folder unless `recursive: true`; notes inside also leave the index. Requires `confirm: true`; `dryRun` lists what would go.

| Argument | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Vault-relative folder path. |
| `confirm` | `true` | yes | Must literally be `true` to execute. Guards against accidental deletion. |
| `recursive` | boolean | no | Delete the folder with everything in it. Default false: refuse a non-empty folder. |
| `dryRun` | boolean | no | If true, report what would be deleted without removing anything. |

### `list_attachments`

List non-markdown files (images, PDFs, ...) with their size and `references`: how many notes link to or embed each one. `unreferencedOnly` finds orphans.

| Argument | Type | Required | Description |
|---|---|---|---|
| `folder` | string | no | Vault-relative folder to list, recursively. Default: the whole vault. |
| `extensions` | array of string | no | Keep only these extensions, case-insensitive, e.g. `["png", ".pdf"]`. |
| `unreferencedOnly` | boolean | no | Return only attachments no note references. |
| `limit` | integer | no | Max results. Default 100. |
| `offset` | integer | no | Results to skip, for paging. Default 0. |

### `create_attachment`

Create a binary file (image, PDF, ...) from base64 content, with any missing parent folders. Max 10 MB decoded. Use `create_note` for markdown.

| Argument | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Vault-relative file path with extension, e.g. `assets/diagram.png`. |
| `content` | string | yes | File bytes, base64-encoded. |
| `overwrite` | boolean | no | Replace an existing file. Default false. |

## Write

### `create_note`

Create a new note in the vault with a title, body, and optional YAML frontmatter. The new note is indexed immediately so semantic search and graph tools can find it. Auto-injects a `title:` field into frontmatter matching the note title unless frontmatter already has one.

| Argument | Type | Required | Description |
|---|---|---|---|
| `title` | string | yes | Note title. Used as the filename base and auto-injected into frontmatter. |
| `content` | string | yes | Markdown body (do not include frontmatter here). |
| `directory` | string | no | Vault-relative subdirectory to create the note in. |
| `frontmatter` | object | no | YAML frontmatter key/value map. `title` is auto-injected unless explicitly set. |

### `create_note_from_template`

Create a note from an Obsidian core Templates template, filling `{{title}}`, `{{date}}`, `{{time}}`, `{{date:FORMAT}}`, `{{time:FORMAT}}` and any `variables`. Templater `<% %>` code is not run and stays as written. Fails if the note exists.

| Argument | Type | Required | Description |
|---|---|---|---|
| `template` | string | yes | Template name in the templates folder, or a vault-relative path. `.md` is optional. |
| `title` | string | yes | Note title. Used as the filename base and for `{{title}}`. |
| `directory` | string | no | Vault-relative subdirectory to create the note in. |
| `variables` | object | no | Extra `{{key}}` substitutions. |
| `date` | string | no | ISO date or date-time to use instead of now. |

### `edit_note`

Modify an existing note. Supports six edit modes: append (add to end; defensively inserts a leading newline if the source didn't end with one), prepend (insert after frontmatter if present, otherwise at file start), replace_window (find a block of text and replace it — optionally fuzzy; fuzzy extends match to consume trailing .?! so the replacement has no doubled punctuation), patch_heading (insert or replace content under a specific heading; `headingOp: 'before' | 'after'` inserts immediately before/after the heading line — use `before` on the NEXT heading to append to a section's end; `headingOp: 'replace'` with `scope: 'section'` (default) replaces to the next same-or-higher heading or EOF — CAREFUL on the LAST heading, this consumes everything below including content separated by blank lines — pass `scope: 'body'` to stop at the first blank line after the body; if the target heading text appears MORE THAN ONCE the call throws MultipleMatches listing each occurrence with its line number — pass `headingIndex: 0 | 1 | ...` (0-indexed, top-to-bottom) to pick one), patch_frontmatter (set a single YAML key; pass `value: null` to clear — or from XML-stringifying clients use `valueJson: 'null'` for true null, `valueJson: 'true'` for a real boolean, `valueJson: '42'` for a number, `valueJson: '["a"]'` for an array; `valueJson` wins over `value` when both are set), at_line (insert or replace at a 1-indexed line number that counts from file start including frontmatter lines). Pass `edits` (array) to apply multiple edits atomically — all succeed or none are written.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Path or fuzzy match of the note to edit. |
| `mode` | `"append"` \| `"prepend"` \| `"replace_window"` \| `"patch_heading"` \| `"patch_frontmatter"` \| `"at_line"` | no | Edit mode. Required unless `edits` array is provided. |
| `content` | string | no | New content to insert or replace (meaning is mode-dependent). |
| `search` | string | no | For `replace_window`: the block of text to locate and replace. |
| `fuzzy` | boolean | no | For `replace_window`: tolerate whitespace and trailing-punctuation drift. |
| `fuzzyThreshold` | number | no | Similarity threshold for fuzzy replace_window matches (0-1, default 0.7). Higher = stricter. 0.9 is recommended for typo-tolerant matching of known-good text. |
| `heading` | string | no | Target heading text for `patch_heading` mode. |
| `headingOp` | `"replace"` \| `"before"` \| `"after"` | no | For `patch_heading`. `replace` (default) replaces section; `before`/`after` inserts adjacent to the heading line. |
| `scope` | `"section"` \| `"body"` | no | For `patch_heading replace`: `section` (default) consumes to next same-or-higher heading; `body` stops at first blank line. |
| `headingIndex` | integer | no | For `patch_heading` when the heading appears more than once — 0-indexed top-to-bottom picker. |
| `key` | string | no | For `patch_frontmatter`: the YAML key to set. |
| `value` | any | no | For `patch_frontmatter`: value to set. Use `null` to clear a key. Prefer `valueJson` from clients that stringify params. |
| `valueJson` | string | no | For `patch_frontmatter`: JSON-encoded value (wins over `value`). Use `"null"` to clear, `"true"` for boolean, `"42"` for number. |
| `line` | integer | no | For `at_line`: 1-indexed line number (counts from file start including frontmatter). |
| `lineOp` | `"before"` \| `"after"` \| `"replace"` | no | For `at_line`: insert before/after the target line, or replace it. Default `replace`. |
| `expectedContent` | string | no | Optional guard for edits that replace text: the exact text being replaced, as last read. If the note changed since, the edit fails and nothing is written. Not with `edits`. |
| `dryRun` | boolean | no | If true, return a unified-diff preview without writing. Pass the returned previewId to apply_edit_preview to commit. |
| `from_buffer` | boolean | no | If true, retry a previously failed replace_window edit using the cached content + search with fuzzy: true, fuzzyThreshold: 0.5. Use when the prior edit failed with NoMatch. Cleared on success. |
| `edits` | array of object | no | Array of edits to apply atomically to a single file. If any edit fails, no edit is written. Applied in order against the accumulated state (each edit sees the result of previous edits in the batch). Use replace_window / patch_heading for content-anchored edits; at_line references the accumulated state, not the original file. |

### `apply_edit_preview`

Apply a previously previewed edit. Pass the `previewId` returned by `edit_note` with `dryRun: true`. Previews expire after 5 minutes. Fails with a descriptive error if the target file changed since the preview was generated — in that case, regenerate the preview and try again.

| Argument | Type | Required | Description |
|---|---|---|---|
| `previewId` | string | yes | The previewId returned by `edit_note` with `dryRun: true`. |

### `link_notes`

Add a wiki-link from one note to another with a context sentence describing why they're connected. Appends the link to the source note and records the edge in the graph so analytics pick it up.

| Argument | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | Source note to add the link from (path or fuzzy match). |
| `target` | string | yes | Target note to link to (path, title, or new wiki-link ref). |
| `context` | string | yes | One-sentence explanation of why these notes are connected. |
| `dryRun` | boolean | no | If true, return the line that would be appended without writing. |

### `move_note`

Rename or move a note. Inbound wiki-links in other notes are rewritten in place immediately (bare [[old]], [[old|alias]], ![[old]] embeds, and [[old#heading]]/[[old^block]] suffixes all handled). If the note's frontmatter has a `title:` field matching the old basename, it's auto-rewritten to the new basename (custom titles and missing titles are left alone). Response includes `linksRewritten: { files, occurrences }`.

| Argument | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | Current path or fuzzy match of the note to move. |
| `destination` | string | yes | New vault-relative path (including `.md`). `.md` is appended automatically if omitted. |
| `dryRun` | boolean | no | If true, report what would be rewritten without mutating any files. |

### `delete_note`

Permanently delete a note. Removes the file from disk AND its index rows (edges, embedding, node). Requires `confirm: true` to guard against accidents. When the delete removes inbound edges, the response is wrapped in a next_actions envelope suggesting a follow-up `rank_notes(method=pagerank, minIncomingLinks=0)` to spot newly orphaned notes.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Path or fuzzy match of the note to delete. |
| `confirm` | `true` | yes | Must literally be `true` to execute. Guards against accidental deletion. |
| `dryRun` | boolean | no | If true, report what would be deleted without removing any files. |

## Properties

### `list_property_values`

List the distinct values of one frontmatter property across notes, with how many times each occurs. Each element of a list value counts separately. Also reports how many notes have the key.

| Argument | Type | Required | Description |
|---|---|---|---|
| `key` | string | yes | Frontmatter key, e.g. `status`. |
| `folder` | string | no | Only notes under this folder. |
| `limit` | integer | no | Max distinct values to return. Default 100. |

### `update_properties`

Set and remove several frontmatter properties of one note in a single write. Creates the frontmatter block if the note has none; the body and keys not named stay as they are. Pass `dryRun: true` to see the resulting frontmatter without writing.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Path or fuzzy match of the note. |
| `set` | object | no | Keys to write, with their values. A `null` value removes the key. |
| `remove` | array of string | no | Keys to remove. Absent keys are skipped. |
| `dryRun` | boolean | no | Return the new frontmatter without writing. |

## Structure

### `vault_overview`

One-call orientation for an unfamiliar vault: note and attachment counts, notes per top-level folder, the most used tags, and the most recently modified notes. Use `list_tags` or `list_notes` for more than the snapshot.

| Argument | Type | Required | Description |
|---|---|---|---|
| `topTags` | integer | no | Tags to include. Default 15. |
| `recent` | integer | no | Recently modified notes to include. Default 10. |

### `list_tags`

List every tag in the vault with the number of notes that carry it. Counts frontmatter `tags`/`tag` and inline `#tags`; tags are returned without the `#`.

| Argument | Type | Required | Description |
|---|---|---|---|
| `sort` | `"count"` \| `"name"` | no | Default `count` (descending). `name` sorts alphabetically. |
| `limit` | integer | no | Max tags to return. Default 100. |
| `prefix` | string | no | Only tags starting with this text, e.g. `project/`. |
| `includeParents` | boolean | no | Default `true`: a note tagged `a/b` also counts toward `a`. |

### `list_bookmarks`

List the vault's Obsidian bookmarks (`.obsidian/bookmarks.json`) as a tree of groups, files, folders, searches, headings and blocks. `totalItems` counts every non-group bookmark.

| Argument | Type | Required | Description |
|---|---|---|---|
| `types` | array of `"file"` \| `"folder"` \| `"search"` \| `"heading"` \| `"block"` \| `"group"` | no | Only these bookmark types. Without `group`, group contents are flattened. |

## Tasks and blocks

### `list_tasks`

List markdown tasks (`- [ ] text`) in the vault, one folder, or one note, ordered by path and line. Each task has its 1-based `line`, raw `status` character, `state` (open/done/cancelled), `text`, nesting `indent`, `parentLine` when nested under another task, and `due` from `📅 YYYY-MM-DD`. Use `line` with `set_task_status` or `ensure_block_id`.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | no | Path or fuzzy match of one note. Omit to scan the vault. |
| `folder` | string | no | Vault-relative folder to scan recursively. |
| `status` | `"open"` \| `"done"` \| `"all"` | no | Default `open`: every status but `x`, `X` and `-`. `done`: `x`/`X`. `all` adds cancelled (`-`). |
| `limit` | integer | no | Max tasks to return. Default 200, max 1000. |
| `offset` | integer | no | Tasks to skip, for paging. Default 0. |

### `set_task_status`

Set the status of one markdown task in place. Only the character between the brackets changes; the rest of the file stays byte-identical. Refuses a line that is not a task.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Path or fuzzy match of the note. |
| `line` | integer | yes | 1-based line of the task, as `list_tasks` returns it. |
| `status` | string | yes | `open` (space), `done` (`x`), or one raw status character such as `/` or `-`. |
| `expectedText` | string | no | The task text you expect on that line. The write is refused when it differs, e.g. after the note changed. |

### `ensure_block_id`

Return the `^block-id` of a block in a note, adding one when it has none, plus a ready `[[Note#^id]]` link. Target the block by `line` or by `heading` (the heading line itself). Placement follows Obsidian: ` ^id` at the end of a paragraph, list item or heading; `^id` on its own line after a table, quote, callout or code block.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Path or fuzzy match of the note. |
| `line` | integer | no | 1-based line inside the block. |
| `heading` | string | no | Heading text; the id goes on that heading line. |
| `id` | string | no | Id to write when the block has none: letters, digits, dashes. Default: random 6 characters. |
| `dryRun` | boolean | no | If true, return the id and diff without writing. |

## Canvas

### `read_canvas`

Read an Obsidian canvas (JSON Canvas) and return its nodes and edges with their ids, positions and content.

| Argument | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Vault-relative path of the `.canvas` file. The extension is optional. |

### `edit_canvas`

Change an Obsidian canvas: `add_node` (creates the canvas if missing; placed right of the right-most node by default), `update_node`, `delete_node` (also removes its edges), `connect` or `disconnect`. Returns the changed node or edge.

| Argument | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Vault-relative path of the `.canvas` file. The extension is optional. |
| `operation` | `"add_node"` \| `"update_node"` \| `"delete_node"` \| `"connect"` \| `"disconnect"` | yes |  |
| `nodeId` | string | no | Node to change (`update_node`, `delete_node`). |
| `type` | `"text"` \| `"file"` \| `"link"` \| `"group"` | no | Node type (`add_node`). |
| `text` | string | no | Markdown content of a `text` node. |
| `file` | string | no | Vault-relative file a `file` node shows. |
| `subpath` | string or null | no | Heading or block of a `file` node, e.g. `#Heading`. `null` removes it. |
| `url` | string | no | URL of a `link` node. |
| `label` | string or null | no | Label of a `group` node or an edge. `null` removes it. |
| `color` | string or null | no | Preset `"1"`-`"6"` or hex like `"#ff0000"`. `null` removes it. |
| `x` | number | no |  |
| `y` | number | no |  |
| `width` | number | no | Default 400. |
| `height` | number | no | Default 200. |
| `fromNode` | string | no | Edge start node id (`connect`). |
| `toNode` | string | no | Edge end node id (`connect`). |
| `fromSide` | `"top"` \| `"right"` \| `"bottom"` \| `"left"` | no |  |
| `toSide` | `"top"` \| `"right"` \| `"bottom"` \| `"left"` | no |  |
| `fromEnd` | `"none"` \| `"arrow"` | no | Default `none`. |
| `toEnd` | `"none"` \| `"arrow"` | no | Default `arrow`. |
| `edgeId` | string | no | Edge to remove (`disconnect`). |

## Graph

### `find_connections`

Find notes linked to (from or to) a given note, up to N hops. Optionally return the full subgraph instead of a flat list. Response is wrapped as `{data, context}` where `context.next_actions` suggests follow-ups like tracing a path to the furthest neighbour via `find_path_between`. Broken-wikilink stub neighbours are excluded by default; pass `includeStubs: true` to include them.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Starting note (path or fuzzy match). |
| `depth` | integer | no | Number of hops to traverse. Default 1, max 3. |
| `returnSubgraph` | boolean | no | Return all edges in the neighborhood as a full subgraph instead of a flat list. |
| `includeStubs` | boolean | no | Default `false`. Set `true` to include broken-wikilink stub neighbours (`frontmatter._stub: true`). |

### `find_path_between`

Find link paths between two notes. Returns all simple paths up to maxDepth edges, optionally including their shared neighbors. Broken-wikilink stub nodes are excluded by default — they are degree-1 dead ends in the undirected graph and will block legitimate paths if left in. Pass `includeStubs: true` to include them.

| Argument | Type | Required | Description |
|---|---|---|---|
| `from` | string | yes | Source note (path or fuzzy match). |
| `to` | string | yes | Target note (path or fuzzy match). |
| `maxDepth` | integer | no | Maximum path length in hops. Default 3. |
| `includeCommon` | boolean | no | Also return notes that both `from` and `to` link to (shared neighbors). |
| `includeStubs` | boolean | no | Default `false`. Set `true` to include broken-wikilink stub nodes (`frontmatter._stub: true`) in the path search. |

### `rank_notes`

Rank notes by importance: 'influence' (densely-connected hubs), 'bridging' (notes that connect otherwise-separate topic clusters), or both. Credibility guards (I): by default, `influence` excludes notes with fewer than `minIncomingLinks: 2` incoming edges — this filters out random-orphan noise that makes PageRank feel meaningless on personal vaults. Pass `minIncomingLinks: 0` to see the unfiltered ranking. Bridging scores are normalized by graph size (divided by n*(n-1)/2) so values compare across vaults of different sizes — a bridging score of 0.5 means the same thing in any vault. Broken-wikilink stub targets are excluded by default; pass `includeStubs: true` to include them.

| Argument | Type | Required | Description |
|---|---|---|---|
| `metric` | `"influence"` \| `"bridging"` \| `"both"` | no | Ranking metric. Default `"both"`. `"influence"` = PageRank; `"bridging"` = betweenness centrality. |
| `limit` | integer | no | Max results to return. Default 20. |
| `includeStubs` | boolean | no | Default `false`. Set `true` to include unresolved wiki-link target stubs (`frontmatter._stub: true`) in the ranked set. With stubs in, popular link targets dominate eigenvector-style centrality even when they have no real content behind them. |
| `minIncomingLinks` | integer | no | Minimum incoming links for influence ranking. Default 2. Pass 0 to see unfiltered PageRank. |

## Maintenance

### `reindex`

Re-index the vault: re-embeds notes whose mtime changed and prunes orphan stubs.

No arguments besides `vault`.

### `index_status`

Report the current state of the vault index: embedder model + dim, count of notes / chunks indexed, chunks skipped during the last reindex (if any), advertised vs discovered max tokens, last reindex reasons, whether a reindex is currently in flight, and the last init error. Read-only — does not mutate anything.

No arguments besides `vault`.

### `find_broken_links`

List wikilinks whose target note is missing, or whose `#heading` / `#^block` is missing from the target. Read-only. Returns source path, 1-based line, link text and reason per broken link.

| Argument | Type | Required | Description |
|---|---|---|---|
| `folder` | string | no | Only scan notes under this folder. |
| `excludeFolders` | array of string | no | Folders to skip. |
| `limit` | integer | no | Max broken links returned. Default 100. |

### `find_orphaned_notes`

List notes with no incoming wikilinks from other notes (self-links do not count). Read-only. Returns path, title and outgoing link count per orphan.

| Argument | Type | Required | Description |
|---|---|---|---|
| `folder` | string | no | Only report notes under this folder. |
| `excludeFolders` | array of string | no | Folders whose notes are not reported. Their links still count. |
| `limit` | integer | no | Max orphans returned. Default 100. |

### `search_and_replace`

Find and replace text across note bodies. Dry run by default: returns per-file match counts and before/after samples; pass `dryRun: false` to write. Skips frontmatter and fenced code unless asked.

| Argument | Type | Required | Description |
|---|---|---|---|
| `pattern` | string | yes | Text to find, or a JavaScript regex when `regex` is true. `^`/`$` match at line ends. |
| `replacement` | string | yes | Replacement text. With `regex`, `$1`, `$<name>` and `$&` expand. |
| `regex` | boolean | no | Treat `pattern` as a regex. Default false. |
| `caseSensitive` | boolean | no | Default true. |
| `folder` | string | no | Only touch notes under this folder. |
| `includeFrontmatter` | boolean | no | Also replace inside YAML frontmatter. Default false. |
| `includeCode` | boolean | no | Also replace inside fenced code blocks. Default false. |
| `dryRun` | boolean | no | Default true. Pass false to write. |
| `maxFiles` | integer | no | Refuse to write when more files match. Default 200. |

### `rename_tag`

Rename a tag across the vault: inline `#tag` (not in code, URLs or headings) and frontmatter `tags`/`tag`. Dry run by default: returns per-file counts; pass `dryRun: false` to write.

| Argument | Type | Required | Description |
|---|---|---|---|
| `from` | string | yes | Tag to rename, `#` optional. Case-insensitive. |
| `to` | string | yes | New tag name, `#` optional. |
| `includeNested` | boolean | no | Also rename `#from/child` to `#to/child`. Default true. |
| `dryRun` | boolean | no | Default true. Pass false to write. |

### `rename_heading`

Rename a heading in a note and rewrite every wikilink and embed that points to it across the vault. Errors if `from` is missing or `to` already exists in the note.

| Argument | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Path or fuzzy match of the note. |
| `from` | string | yes | Current heading text, without `#`. |
| `to` | string | yes | New heading text, without `#`. |
| `dryRun` | boolean | no | If true, report the changes without writing. Default false. |
