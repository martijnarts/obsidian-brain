# Running as a service (macOS)

A client that starts `obsidian-brain server` over stdio needs no service: the server watches its vaults while it runs. This page covers two LaunchAgents for a Mac where the server must run on its own. LaunchAgents need no root.

launchd runs jobs with a minimal `PATH`, so give the binary by its absolute path. Replace `/Users/you/.nix-profile/bin/obsidian-brain` with the output of `command -v obsidian-brain`, and the vault paths with yours.

## A long-running HTTP server

The HTTP server indexes its vaults at startup and watches them while it runs. Save this as `~/Library/LaunchAgents/com.you.obsidian-brain.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.you.obsidian-brain</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/you/.nix-profile/bin/obsidian-brain</string>
        <string>server</string>
        <string>--transport</string>
        <string>http</string>
        <string>--listen</string>
        <string>127.0.0.1:8091</string>
        <string>--vault</string>
        <string>notes=/Users/you/vaults/notes</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/tmp/obsidian-brain.log</string>
</dict>
</plist>
```

Load it:

```bash
launchctl load ~/Library/LaunchAgents/com.you.obsidian-brain.plist
launchctl list | grep obsidian-brain
```

The server has no authentication. Keep it on loopback and put an authenticating proxy in front of it.

Do not run `obsidian-brain watch` as a LaunchAgent: it stops when its standard input closes, and launchd gives a job a closed input.

## An index on a timer

Use this when no server runs all the time. The `index` command is incremental, so each run is cheap. Save this as `~/Library/LaunchAgents/com.you.obsidian-brain-index.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.you.obsidian-brain-index</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/you/.nix-profile/bin/obsidian-brain</string>
        <string>index</string>
        <string>--vault</string>
        <string>notes=/Users/you/vaults/notes</string>
    </array>
    <key>StartInterval</key>
    <integer>1800</integer>
    <key>StandardOutPath</key>
    <string>/tmp/obsidian-brain-index.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/obsidian-brain-index.err</string>
</dict>
</plist>
```

`StartInterval` is in seconds. The job updates the index a server uses when both use the same `DATA_DIR` and the same vault name.

## Environment and changes

Add an `EnvironmentVariables` dictionary for the variables in [Configuration](configuration.md). After a change to a plist, unload and load it again:

```bash
launchctl unload ~/Library/LaunchAgents/com.you.obsidian-brain.plist
launchctl load ~/Library/LaunchAgents/com.you.obsidian-brain.plist
```

## Full Disk Access

A vault under `~/Documents`, `~/Desktop` or iCloud Drive needs Full Disk Access for the process that reads it. Grant it to the binary in System Settings, Privacy and Security, Full Disk Access, or move the vault elsewhere.
