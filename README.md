# garmin-connector

Give Claude read access to your Garmin **sleep**, **HRV** and **Body Battery** data, so your
morning briefing can tell you how you recovered and how to approach the day.

Written in Rust. Runs on your own machine. Your Garmin data and login tokens never leave it,
except for the requests to Garmin itself.

> **Unofficial.** Garmin has no API for personal use, so this logs in the way the Garmin
> Connect mobile app does. It is not affiliated with or endorsed by Garmin, and it can break
> when Garmin changes its login. Use it with your own account only.

## What's in here

| Crate | What it is |
|-------|------------|
| [`garmin-connect`](crates/garmin-connect) | A reusable async Rust client for Garmin Connect: login (with two-factor), token refresh, and typed sleep / HRV / Body Battery data. No MCP or Claude specifics. |
| [`garmin-mcp`](crates/garmin-mcp) | An [MCP](https://modelcontextprotocol.io) server and CLI built on the client. Keeps a local JSON history and serves it to Claude. |

## Install

You need a Rust toolchain ([rustup.rs](https://rustup.rs)).

```bash
cargo install --path crates/garmin-mcp
```

This puts `garmin-mcp` in `~/.cargo/bin`.

## Log in (once)

```bash
garmin-mcp login
```

You're asked for your Garmin email, your password (hidden as you type), and a two-factor code
if your account uses one. Only the resulting tokens are saved, never the password. The token
file is readable only by your user. The access token renews itself; you only need to log in
again if Garmin revokes the session.

Check that it worked, and see what Claude will see:

```bash
garmin-mcp status
garmin-mcp today
```

The first `today` downloads 29 days (last night plus a 28-day baseline), which takes a few seconds.

## Connect it to Claude Desktop

Open Claude Desktop's config file:

```text
~/Library/Application Support/Claude/claude_desktop_config.json
```

Add the server under `mcpServers` (use the full path to the binary, since Claude Desktop does
not read your shell's `PATH`):

```json
{
  "mcpServers": {
    "garmin": {
      "command": "/Users/YOU/.cargo/bin/garmin-mcp",
      "args": ["serve"]
    }
  }
}
```

Restart Claude Desktop. In a new chat, ask *"How did I sleep last night?"*

## Tools Claude gets

| Tool | Use |
|------|-----|
| `get_recovery_snapshot` | Last night's sleep (hours, score, stages, bed and wake time, resting HR), HRV (last night, weekly average, status, your balanced range) and Body Battery (now, peak, low), next to your own 28-day baseline and the difference from it. Built for a daily briefing. |
| `get_recovery_history` | One row per day for up to 92 days, for trends and looking for patterns. |
| `get_garmin_raw` | Garmin's original JSON for one dataset and day, for fields the summaries leave out. |

Dates are the day you woke up: the night of 6 to 7 November is stored as 7 November.

### Using it in a scheduled briefing

Add something like this to your briefing prompt:

> Call `get_recovery_snapshot`. In two or three sentences, tell me how I recovered compared
> with my baseline, and whether today suits hard training or something easier. Check
> yesterday's training in Strava.

## Where your data lives

Everything is in one folder, by default `~/Library/Application Support/garmin-connector/`
on macOS (change it with `--home` or `GARMIN_CONNECTOR_HOME`):

```text
garmin-connector/
  tokens.json                 login tokens (owner-only permissions)
  data/
    sleep/2026-11-07.json
    hrv/2026-11-07.json
    body_battery/2026-11-07.json
```

Each day file holds Garmin's original response plus the summary extracted from it, so
nothing is lost if you later want a field that isn't extracted today. Days older than two
days are final and are never downloaded again. Recent days are refreshed when the stored copy
is more than 30 minutes old. `garmin-mcp sync --days 90` backfills a longer history.

## Commands

```text
garmin-mcp serve        run the MCP server (what Claude Desktop starts)
garmin-mcp login        log in with email + password (+ two-factor code)
garmin-mcp logout       delete saved tokens
garmin-mcp status       data folder and login state
garmin-mcp today        print today's snapshot as JSON
garmin-mcp sync         download the last N days (default 28)
```

`--china` (or `GARMIN_CONNECTOR_CHINA=1`) uses garmin.cn for accounts registered in China.
Set `RUST_LOG=debug` to see what is happening (logs go to stderr).

## If login fails

- **Rate limited (429).** Garmin limits login attempts per network. Wait an hour or two before
  trying again. Repeated attempts make it worse.
- **Blocked by bot protection (403 or captcha).** Cloudflare didn't like the connection.
  Garmin's Python community library gets past this by imitating a phone's TLS handshake. If
  this happens to you, please open an issue: the fix is to add the same here.
- **Wrong password.** Check that you can sign in at connect.garmin.com.

## Development

```bash
cargo test            # all tests use a mock Garmin server; nothing touches the network
cargo clippy --all-targets
```

The login flow follows the maintained Python library
[python-garminconnect](https://github.com/cyberjunky/python-garminconnect). When Garmin
changes something, its changelog is the first place to look. The constants are in
[`crates/garmin-connect/src/endpoints.rs`](crates/garmin-connect/src/endpoints.rs).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
