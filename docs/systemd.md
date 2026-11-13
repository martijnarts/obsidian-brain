# Running as a service (Linux)

A client that starts `obsidian-brain server` over stdio needs no service: the server watches its vaults while it runs. This page covers two setups for a machine where the server must run on its own. Both are systemd user units under `~/.config/systemd/user/`, with no root.

Replace `/home/you/.nix-profile/bin/obsidian-brain` with the output of `command -v obsidian-brain`, and the vault paths with yours.

## A long-running HTTP server

The HTTP server indexes its vaults at startup and watches them while it runs. Create `~/.config/systemd/user/obsidian-brain.service`:

```ini
[Unit]
Description=obsidian-brain MCP server
After=network.target

[Service]
ExecStart=/home/you/.nix-profile/bin/obsidian-brain server \
  --transport http --listen 127.0.0.1:8091 \
  --vault personal=/home/you/vaults/personal
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

Then:

```bash
systemctl --user daemon-reload
systemctl --user enable --now obsidian-brain.service
journalctl --user -u obsidian-brain.service -f
```

The server has no authentication. Keep it on loopback and put an authenticating proxy in front of it. If the proxy sends its public host name as the `Host` header, add `--allowed-host <name>`.

To keep the service running after you log out, run `sudo loginctl enable-linger $USER` once.

Do not run `obsidian-brain watch` as a service: it stops when its standard input closes, and systemd gives a service a closed input.

## An index on a timer

Use this when the watcher misses changes, for example on SMB or NFS, or when no server runs all the time. The `index` command is incremental, so each run is cheap.

`~/.config/systemd/user/obsidian-brain-index.service`:

```ini
[Unit]
Description=obsidian-brain incremental index

[Service]
Type=oneshot
ExecStart=/home/you/.nix-profile/bin/obsidian-brain index --vault personal=/home/you/vaults/personal
```

`~/.config/systemd/user/obsidian-brain-index.timer`:

```ini
[Unit]
Description=Run the obsidian-brain index every 30 minutes

[Timer]
OnBootSec=5min
OnUnitActiveSec=30min
Persistent=true

[Install]
WantedBy=timers.target
```

Then:

```bash
systemctl --user daemon-reload
systemctl --user enable --now obsidian-brain-index.timer
systemctl --user list-timers obsidian-brain-index.timer
```

`systemctl --user start obsidian-brain-index.service` runs it at once. The timer updates the index a server uses when both use the same `DATA_DIR` and the same vault name. Set `OBSIDIAN_BRAIN_NO_WATCH=1` on that server if the timer replaces its watcher.

## Environment

Add `Environment=` lines to `[Service]` for the variables in [Configuration](configuration.md), or `EnvironmentFile=` with a file in the format of `.env.example`.

## Remove

```bash
systemctl --user disable --now obsidian-brain.service obsidian-brain-index.timer
rm ~/.config/systemd/user/obsidian-brain*.{service,timer}
systemctl --user daemon-reload
```
