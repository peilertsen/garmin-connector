# garmin-connector

[![CI](https://github.com/peilertsen/garmin-connector/actions/workflows/ci.yml/badge.svg)](https://github.com/peilertsen/garmin-connector/actions/workflows/ci.yml)

Give Claude read access to your Garmin **sleep**, **HRV**, **Training Readiness**, **Body
Battery**, **stress**, **resting heart rate**, **SpO2** and **respiration** data, so your morning
briefing can tell you how you recovered and how to approach the day.

Written in Rust. Runs on your own machine. Your Garmin data and login tokens never leave it,
except for the requests to Garmin itself.

> **Unofficial.** Garmin has no API for personal use, so this logs in the way the Garmin
> Connect mobile app does. It is not affiliated with or endorsed by Garmin, and it can break
> when Garmin changes its login. Use it with your own account only.

## What's in here

| Crate | What it is |
|-------|------------|
| [`garmin-connect`](crates/garmin-connect) | A reusable async Rust client for Garmin Connect: login (with two-factor), token refresh, and typed sleep, HRV, Body Battery, daily summary (stress, resting HR, SpO2, respiration) and Training Readiness data. No MCP or Claude specifics. |
| [`garmin-mcp`](crates/garmin-mcp) | An [MCP](https://modelcontextprotocol.io) server and CLI built on the client. Keeps a local JSON history and serves it to Claude. |

## Install

You need a Rust toolchain ([rustup.rs](https://rustup.rs)).

```bash
cargo install --locked --git https://github.com/peilertsen/garmin-connector garmin-mcp
```

Or, from a clone of this repository:

```bash
cargo install --locked --path crates/garmin-mcp
```

Either way, `garmin-mcp` ends up in `~/.cargo/bin`.

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

The first `today` downloads 29 days (last night plus a 28-day baseline), about 120 requests, which takes
under a minute. After that only new or recent days are fetched.

## Connect it to Claude Desktop

**Quit Claude Desktop first (⌘Q).** It reads its config only at startup and writes its own
copy back while running, so changes made while it is open get overwritten.

Then open its config file:

```text
macOS:    ~/Library/Application Support/Claude/claude_desktop_config.json
Windows:  %APPDATA%\Claude\claude_desktop_config.json
```

Add the server under `mcpServers`. Use the full path to the binary, because Claude Desktop
does not read your shell's `PATH`:

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

On Windows the command is `C:\\Users\\YOU\\.cargo\\bin\\garmin-mcp.exe` (with the backslashes
doubled, as JSON requires).

Start Claude Desktop again. `garmin` should be listed under **Settings → Developer**. In a new
chat, ask *"How did I sleep last night?"* If it doesn't appear, the server's log is at
`~/Library/Logs/Claude/mcp-server-garmin.log`.

## Tools Claude gets

| Tool | Use |
|------|-----|
| `get_recovery_snapshot` | Last night's sleep (hours, score, stages, bed and wake time), HRV (last night, weekly average, status, your balanced range), Training Readiness (score, level, recovery time, contributing factors), Body Battery (now, at wake, peak, low, and what charged or drained it), stress (today so far and yesterday), resting heart rate, SpO2 and respiration, next to your own 28-day baseline and the difference from it. Built for a daily briefing. |
| `get_recovery_history` | One row per day for up to 92 days, for trends and looking for patterns. |
| `get_garmin_raw` | Garmin's original JSON for one dataset and day, for fields the summaries leave out. |

Dates are the day you woke up: the night of 6 to 7 November is stored as 7 November.

### Using it in a scheduled briefing

Add something like this to your briefing prompt:

> Call `get_recovery_snapshot`. In two or three sentences, tell me how I recovered compared
> with my baseline, and whether today suits hard training or something easier. Check
> yesterday's training in Strava.

## Where your data lives

Everything is in one folder. Change it with `--home` or `GARMIN_CONNECTOR_HOME`; the defaults are:

```text
macOS:    ~/Library/Application Support/garmin-connector/
Linux:    ~/.local/share/garmin-connector/   (or $XDG_DATA_HOME/garmin-connector/)
Windows:  %APPDATA%\garmin-connector\
```

`garmin-mcp status` prints the folder in use. It is laid out like this:

```text
garmin-connector/
  tokens.json                 login tokens (owner-only permissions on macOS and Linux)
  data/
    sleep/2026-11-07.json
    hrv/2026-11-07.json
    body_battery/2026-11-07.json
    daily_summary/2026-11-07.json       stress, resting HR, SpO2, respiration, Body Battery events
    training_readiness/2026-11-07.json
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

Developed and used on macOS. CI also runs the tests on Linux. Windows should work but is untested.

When Garmin changes something, the changelog of python-garminconnect (see below) is the first
place to look. The client identity and URLs are in
[`crates/garmin-connect/src/endpoints.rs`](crates/garmin-connect/src/endpoints.rs).

Never paste real Garmin responses, tokens or log output into issues or test fixtures without
removing personal data first. The fixtures in `crates/garmin-connect/tests/fixtures` are
made up.

## Acknowledgements

The login flow, client identifiers and endpoints follow
[python-garminconnect](https://github.com/cyberjunky/python-garminconnect) by Ron Klinkien
and contributors (MIT licensed). Its maintainers do the hard work of keeping up with Garmin's
login changes. This project is a separate Rust implementation and shares no code with it.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution you submit for inclusion in this
project is dual licensed as above, without any additional terms or conditions.
