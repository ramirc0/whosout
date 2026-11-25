# whosout

A calendar of who is out, read from a Slack channel where a team posts its time off.

## Description

Teams often report time off as free text in one Slack channel: "WFH today", "taking 11/24 and 11/25 off (VAC)", "sick day (SIC)". `whosout` reads that channel and draws a roster with one row per person and one column per weekday.

- `whosout listen` holds a Slack Socket Mode connection. It pulls the whole channel on connect and again on every new, edited or deleted message.
- `whosout serve` serves the calendar page and its JSON at `/api/calendar?first=YYYY-MM-DD&last=YYYY-MM-DD`.
- `whosout sync` pulls the channel once.
- `whosout pending` prints the messages that need a human reading.

Each message is read by strict regexes into a kind and a span of days. The kinds are `WFH` (working from home), `SIC` (sick), `VAC` (vacation), `PERS` (personal) and `OUT` (time off with no code, travel or a conference). The reading accepts a message only when it finds one kind and understands every date word in it. It refuses the rest, and `whosout pending` lists them with the reason.

You resolve a refused message by hand in `resolutions.toml` in the data directory:

```toml
[[resolution]]
message = "1700000000.000100"   # the Slack timestamp that `whosout pending` prints
why = "PERS on Monday, VAC on Tuesday and Wednesday"
absences = [
  { kind = "PERS", first = "2026-11-23", last = "2026-11-23" },
  { kind = "VAC", first = "2026-11-24", last = "2026-11-25" },
]
```

An empty `absences` marks chatter or a cancelled post. `part` is optional and takes `morning`, `afternoon` or `half`.

The data directory also holds `whosout.db`, a SQLite file. Its `absence_days` table has one row per person per weekday away, ready for analysis.

## Getting Started

### Dependencies

* A Slack app in your workspace with:
  * Socket Mode enabled, and an app-level token (`xapp-...`) with `connections:write`.
  * The user token scopes `channels:history` and `users:read`, and the resulting user token (`xoxp-...`).
  * Event Subscriptions enabled, with `message.channels` under "Subscribe to events on behalf of users".
* The user who installs the app must be a member of the status channel, and the channel must be public.
* Docker with Compose, or Rust 1.100 and [just](https://github.com/casey/just).

### Installing

```
git clone https://github.com/ramirc0/whosout
cd whosout
cp .env.example .env
```

Fill in `.env`. It names every setting.

### Executing program

With Docker Compose:

```
docker compose up -d --build
```

The calendar is then at http://127.0.0.1:8377. The data lives in the `whosout_data` volume.

As systemd user units on Linux:

```
mkdir -p ~/.config/whosout && cp .env ~/.config/whosout/env && chmod 600 ~/.config/whosout/env
just install
```

Directly:

```
just run sync
just run serve
```

## Help

* Run only one `whosout listen` per Slack app. Slack sends each event to one open connection, so a second listener takes events from the first.
* Slack with a user subscription sends the listener the message events of every public channel that user is in. `whosout` acknowledges and drops all but the status channel.
* The page shows how many messages need review. `whosout pending` fails when `resolutions.toml` is malformed or names an unknown message.
* The page does not refresh itself. Reload it to see a new post.

```
whosout --help
```

## Authors

Christian Ramirez

## Acknowledgments

* The page embeds [Hanken Grotesk](https://github.com/marcologous/hanken-grotesk) under the SIL Open Font License. See `web/HankenGrotesk-OFL.txt`.
