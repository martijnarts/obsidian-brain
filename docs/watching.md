# Live updates

`obsidian-brain server` watches each vault and reindexes a note shortly after it changes. The watcher uses the notify crate: inotify on Linux, FSEvents on macOS. It does not poll, so an idle watcher costs no CPU.

## What it reacts to

- A new or changed `.md` file: the server parses it, embeds its changed chunks and updates its node and links.
- A deleted `.md` file: the server deletes its node and links.

The watcher skips every other file type and the directories `.obsidian`, `.trash`, `.git`, `node_modules` and `attachments`. Changes made while the watcher was not running are not replayed; the catch-up index at server startup finds them.

## Timing

A change waits for two things:

1. The file must have no writes for 2 seconds, so a half-written file is never indexed.
2. Then a per-file debounce of 3 seconds collapses a burst of saves into one reindex.

The debounce is per path. Three saves of one note and one save of another give two reindexes. Obsidian saves about 2 seconds after you stop typing, so a long editing session on one note gives one reindex per pause. A delete waits for the debounce only.

## Settings

| Variable | Default | Effect |
|---|---|---|
| `OBSIDIAN_BRAIN_NO_WATCH` | unset | `1` starts no watcher in `server`. |
| `OBSIDIAN_BRAIN_WATCH_DEBOUNCE_MS` | `3000` | The per-file debounce of the watcher in `server`. |
| `OBSIDIAN_BRAIN_NO_CATCHUP` | unset | `1` skips the catch-up index at `server` startup. |

Turn the watcher off for a vault on SMB or NFS, where inotify and FSEvents miss changes, and run `obsidian-brain index` on a timer instead. [Running as a service](systemd.md) shows a timer.

## The `watch` command

`obsidian-brain watch --vault notes=/path/to/vault` runs the same watcher with no MCP transport. It takes `--debounce <ms>` in place of the environment variable. It keeps a server's index current when it uses the same `DATA_DIR` and the same vault name as the server.

`watch` stops when its standard input closes, so it suits a terminal or a parent process that holds its input open. A service manager gives a service a closed input, so `watch` exits at once there. For a long-running process under systemd or launchd, run `server --transport http`, which watches its vaults too, or run `index` on a timer.

If edits do not reach the index, see [Troubleshooting](troubleshooting.md#watcher-not-firing).
