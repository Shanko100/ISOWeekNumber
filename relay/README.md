# WeekNumber game relay

A Cloudflare Worker that powers WeekNumber's online games of its hidden extra: the public
lobby list, plus a relay that passes messages between a host and its players.
It runs no game rules. The hosting app stays authoritative, exactly as on a
local network.

- `Directory` (one Durable Object) keeps the public lobby list. App windows
  showing the Online tab get it over a WebSocket and see changes live.
- `Lobby` (one Durable Object per lobby) relays one game between its host and
  up to five players.

Every socket uses the WebSocket hibernation API, so a lobby waiting between
moves costs nothing. Both classes are SQLite-backed, which the Workers Free
plan requires.

## Cost

The Workers Free plan is enough. As of October 2026 it allows:

- 100,000 requests a day.
- Incoming WebSocket messages, counted 20:1. A 4-player game of ~600 moves
  costs about 35 requests; opening the lobby list costs 1.
- 13,000 GB-s of Durable Object duration a day.

If a daily limit is reached, online play stops until 00:00 UTC; nothing is
billed.

## Abuse limits

Lobbies are public and open, so the Worker limits what one network can do:

- 5 lobby creations per 10 minutes, and at most 3 open lobbies.
- 200 lobbies in total, 6 seats per lobby.
- 64 KiB per message, and 24-character names.
- A lobby whose host never connects is dropped after 2 minutes; every lobby
  is dropped after 12 hours.

Rate limits key on a hash of the caller's IP address, salted with a random
value generated inside the Directory object, so the hashes can't be
reversed. Raw addresses are never stored, and hosts learn nothing about
their players' networks: a player reaches the host only as "peer n".

## Privacy

The public relay at `https://tzd-relay.weeknumber.workers.dev` runs on
Cloudflare Workers for this project. It has no accounts, cookies, analytics
or tracking. The app only connects to it while the online lobby list or an
online game is open.

| What | Why | How long |
|---|---|---|
| Your IP address | Abuse limits | Never stored as is. A salted 16-character hash is kept when you create a lobby: with the lobby until it closes, and for the creation limit, deleted within 15 minutes. |
| Lobby name and host name | Shown in the public lobby list | Until the lobby closes; at most about 12 hours. |
| Player names, game moves and chat | Passed between the host and the players | Not stored; each message is forwarded and dropped. A closed lobby's storage is wiped. |

Everything is encrypted in transit (TLS). Names and game moves are readable
by the relay, which checks them before forwarding, so don't put anything
private in a name. Chat is end-to-end encrypted by the app: the relay only
sees who sent a sealed message, when, and roughly how long it is.

Cloudflare, which runs the relay, also keeps request logs (time, address
requested, status and connection details, which may include your IP
address) for up to 3 days; see
[Cloudflare's privacy policy](https://www.cloudflare.com/privacypolicy/).

Questions or requests about your data: open an issue in this repository.
Because raw addresses are never stored and names are gone within 12 hours,
there is usually nothing left to look up or delete.

## Develop

```sh
cd relay
npm install
npm run check                 # TypeScript type check
npx wrangler dev --port 8787 --var CREATE_LIMIT:10000 --var LOBBIES_PER_CLIENT:10000
```

Point the app or the integration tests at the local relay:

```sh
WEEKNUMBER_TZD_RELAY=http://127.0.0.1:8787 python src/weeknumber/weeknumber.py
WEEKNUMBER_TZD_RELAY=http://127.0.0.1:8787 python -m pytest tests/test_tzd_relay.py
```

The app takes a plain `http://` address only for a relay on the same
machine (`localhost`, `127.0.0.1`, `::1`); any other relay needs `https://`.

## Deploy

```sh
cd relay
npx wrangler login            # once, opens the browser
npx wrangler deploy
```

The relay is deployed at `https://tzd-relay.weeknumber.workers.dev`, which is
`RELAY_URL` in `src/weeknumber/weeknumber_tzd_relay.py`. Changing the
account's workers.dev subdomain changes that address, so update `RELAY_URL`
too; apps built with the old one lose the Online tab's connection. A build
with `RELAY_URL` empty hides the Online tab.

Deploying a new version disconnects open games, so deploy when nobody is
playing.
