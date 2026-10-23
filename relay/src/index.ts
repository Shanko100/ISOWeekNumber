/**
 * Timezone Dominator relay: the public lobby list and message relay behind WeekNumber's
 * online games.
 *
 * The relay never runs game rules. The hosting app stays authoritative, as on
 * a local network: it rolls the dice and sends every player their own view of
 * the state. The relay only lists lobbies and passes messages between the
 * host and its players, so the app's protocol, validation and rules are the
 * same for LAN and online play.
 *
 *   GET  /v1/lobbies              WebSocket: live public lobby list
 *                                 (plain GET returns the list as JSON)
 *   POST /v1/lobbies              create a lobby -> { id, token, features }
 *   GET  /v1/lobbies/:id/host     WebSocket for the host (Bearer token)
 *   GET  /v1/lobbies/:id/join     WebSocket for a player
 *
 * Host socket frames are JSON envelopes:
 *   relay -> host  {"relay":"open","peer":n} | {"relay":"data","peer":n,"data":s}
 *                  | {"relay":"close","peer":n}
 *   host -> relay  {"relay":"send","peer":n,"data":s} | {"relay":"close","peer":n}
 *                  | {"relay":"info","players":n,"started":b}
 * Player sockets carry the app's own messages unchanged, one per frame. Only
 * messages in the game's protocol are forwarded, with a per-player rate cap
 * and a per-lobby byte budget, so the relay is no use for anything but Timezone Dominator.
 *
 * Chat ("chat" messages) is end-to-end encrypted by the app; the relay only
 * sees sealed boxes. It has its own limits that drop excess chat instead of
 * ending the game, and new lobbies advertise it as a feature so the app
 * never sends chat through an older relay that would refuse it.
 *
 * One Directory object holds the list. A single object is the documented
 * anti-pattern at large scale, but here it only sees lobby creations, a few
 * status updates per game and one connection per open lobby browser.
 * Each lobby is its own object. All sockets use the hibernation API, so idle
 * lobbies cost no duration.
 */
import { DurableObject } from "cloudflare:workers";

export interface Env {
  DIRECTORY: DurableObjectNamespace<Directory>;
  LOBBY: DurableObjectNamespace<Lobby>;
  /** Optional overrides for local testing: wrangler dev --var CREATE_LIMIT:1000 */
  CREATE_LIMIT?: string;
  LOBBIES_PER_CLIENT?: string;
  GUEST_RATE?: string;
  CHAT_RATE?: string;
  CHAT_BUDGET?: string;
}

function limit(value: string | undefined, fallback: number): number {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : fallback;
}

const MAX_NAME_CHARS = 24;
/** Matches MAX_MESSAGE_BYTES in weeknumber_tzd_net.py. */
const MAX_MESSAGE_CHARS = 64 * 1024;
const MAX_HOST_FRAME_CHARS = MAX_MESSAGE_CHARS + 1024;
/** Players send hellos, colour picks, game actions and sealed chat: small. */
const MAX_GUEST_MESSAGE_CHARS = 8 * 1024;
/** The game's own message types. Anything else is not Timezone Dominator, so the relay
 *  refuses it -- it must not work as a general-purpose message relay. */
const GUEST_TYPES = new Set(["hello", "color", "action", "chat", "bye"]);
const HOST_TYPES = new Set(["welcome", "lobby", "state", "error", "chat", "bye"]);
/** What this relay passes beyond the game itself, reported on lobby creation. */
const FEATURES = ["chat"];
/** Chat: one sealed box of at most 280 characters is under 2 KiB. Over the
 *  soft rate a player's chat is dropped; only far beyond it are they kicked. */
const MAX_GUEST_CHAT_CHARS = 3 * 1024;
const CHAT_SOFT_PER_WINDOW = 20;
const CHAT_HARD_PER_WINDOW = 60;
const CHAT_WINDOW_MS = 10_000;
/** Per lobby, in memory: chat stops when spent, the game carries on. */
const CHAT_BYTE_BUDGET = 8 * 1024 * 1024;
/** Per player: actions are human-paced, so this is generous for a game. */
const GUEST_MESSAGES_PER_WINDOW = 60;
const GUEST_WINDOW_MS = 10_000;
/** Per lobby: a long six-player game moves roughly 30 MB of state views. */
const LOBBY_BYTE_BUDGET = 96 * 1024 * 1024;
const BUDGET_FLUSH_EVERY = 100;
/** Six seats in all: the host plus five players. */
const MAX_GUESTS = 5;
const MAX_LOBBIES = 200;
const MAX_LOBBIES_PER_CLIENT = 3;
const CREATE_LIMIT = 5;
const CREATE_WINDOW_MS = 10 * 60_000;
/** A lobby whose host never connects is dropped after this. */
const UNCLAIMED_MS = 2 * 60_000;
const MAX_AGE_MS = 12 * 60 * 60_000;
const SWEEP_MS = 5 * 60_000;
const MAX_VIEWERS = 2000;
const LOBBY_ID = /^[a-z0-9]{12}$/;
const BOARD_FINGERPRINT = /^[0-9a-f]{16}$/;

interface LobbyInfo {
  id: string;
  name: string;
  host: string;
  board: string;
  players: number;
  started: boolean;
  created: number;
}

type Attachment = { role: "host" } | { role: "guest"; peer: number };

/** The type of a game message -- a JSON object with the protocol version and
 *  a known type -- or null for anything else. */
function gameMessageType(text: string, types: Set<string>): string | null {
  let message: unknown;
  try {
    message = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof message !== "object" || message === null || Array.isArray(message)) return null;
  const { v, type } = message as { v?: unknown; type?: unknown };
  return v === 1 && typeof type === "string" && types.has(type) ? type : null;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });
}

/** Printable, single-line, bounded -- the same rule as clean_name() in the app. */
function cleanName(value: unknown, fallback: string): string {
  if (typeof value !== "string") return fallback;
  const text = value
    .replace(/\p{C}/gu, "")
    .replace(/\p{Z}/gu, " ")
    .replace(/\s+/g, " ")
    .trim();
  return Array.from(text).slice(0, MAX_NAME_CHARS).join("") || fallback;
}

function randomString(length: number, alphabet: string): string {
  const bytes = crypto.getRandomValues(new Uint8Array(length));
  return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join("");
}

async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

function callerIp(request: Request): string {
  return request.headers.get("CF-Connecting-IP") ?? "local";
}

function isWebSocket(request: Request): boolean {
  return request.headers.get("Upgrade")?.toLowerCase() === "websocket";
}

function isOpen(ws: WebSocket): boolean {
  return ws.readyState === WebSocket.OPEN;
}

function safeClose(ws: WebSocket, code: number, reason: string): void {
  try {
    ws.close(code, reason);
  } catch {
    // Already closing or closed.
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const parts = url.pathname.split("/").filter(Boolean);
    if (parts[0] !== "v1" || parts[1] !== "lobbies" || parts.length > 4) {
      return json({ error: "Not found." }, 404);
    }
    const directory = env.DIRECTORY.getByName("directory");

    if (parts.length === 2) {
      if (request.method === "GET" && isWebSocket(request)) return directory.fetch(request);
      if (request.method === "GET") return json({ lobbies: await directory.list() });
      if (request.method === "POST") {
        const length = Number(request.headers.get("content-length") ?? "0");
        if (length > 2048) return json({ error: "Request too large." }, 413);
        let body: Record<string, unknown>;
        try {
          body = await request.json();
        } catch {
          return json({ error: "Malformed request." }, 400);
        }
        const result = await directory.create(
          callerIp(request), body?.name, body?.host, body?.board);
        return "error" in result ? json({ error: result.error }, result.status)
                                 : json(result, 201);
      }
      return json({ error: "Method not allowed." }, 405);
    }

    if (parts.length === 4 && LOBBY_ID.test(parts[2]) &&
        (parts[3] === "host" || parts[3] === "join")) {
      if (request.method !== "GET" || !isWebSocket(request)) {
        return new Response("Expected a WebSocket upgrade.", { status: 426 });
      }
      // Players reach the host as "peer n" only: nothing about their network.
      return env.LOBBY.getByName(parts[2]).fetch(request);
    }
    return json({ error: "Not found." }, 404);
  },
} satisfies ExportedHandler<Env>;

// ---------------------------------------------------------------------------
// Directory: the public lobby list
// ---------------------------------------------------------------------------
export class Directory extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      ctx.storage.sql.exec(`
        CREATE TABLE IF NOT EXISTS lobbies (
          id TEXT PRIMARY KEY,
          name TEXT NOT NULL,
          host TEXT NOT NULL,
          board TEXT NOT NULL,
          players INTEGER NOT NULL DEFAULT 1,
          started INTEGER NOT NULL DEFAULT 0,
          claimed INTEGER NOT NULL DEFAULT 0,
          created INTEGER NOT NULL,
          client TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS creations (client TEXT NOT NULL, at INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS creations_by_client ON creations (client, at);
        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
      `);
      // A random salt that never leaves this object. The source is public, so
      // a constant salt would let anyone reverse the hashes by trying every
      // IPv4 address.
      ctx.storage.sql.exec(
        "INSERT OR IGNORE INTO meta (key, value) VALUES ('salt', ?)",
        randomString(32, "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"));
    });
  }

  /** The rate-limit identity for an IP: a keyed hash, never the address. */
  private async clientKey(ip: string): Promise<string> {
    const salt = this.ctx.storage.sql.exec<{ value: string }>(
      "SELECT value FROM meta WHERE key = 'salt'").one().value;
    return (await sha256Hex(`${salt}:${ip}`)).slice(0, 16);
  }

  /** A lobby browser: the list now, then again on every change. */
  async fetch(request: Request): Promise<Response> {
    if (this.ctx.getWebSockets().length >= MAX_VIEWERS) {
      return new Response("Too many lobby browsers are open; try again shortly.", { status: 503 });
    }
    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server);
    server.send(this.snapshot());
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket): Promise<void> {
    // Browsers only listen. Anything else is misuse, and messages cost requests.
    safeClose(ws, 1008, "This socket is receive-only.");
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    safeClose(ws, 1000, "Bye.");
  }

  list(): LobbyInfo[] {
    return this.ctx.storage.sql.exec<{
      id: string; name: string; host: string; board: string;
      players: number; started: number; created: number;
    }>(`SELECT id, name, host, board, players, started, created FROM lobbies
        WHERE claimed = 1 ORDER BY started ASC, created DESC`)
      .toArray()
      .map((row) => ({ ...row, started: row.started === 1 }));
  }

  private snapshot(): string {
    return JSON.stringify({ v: 1, type: "lobbies", lobbies: this.list() });
  }

  private broadcast(): void {
    const message = this.snapshot();
    for (const ws of this.ctx.getWebSockets()) {
      if (isOpen(ws)) {
        try {
          ws.send(message);
        } catch {
          // A browser that went away; its close event cleans up.
        }
      }
    }
  }

  async create(ip: string, name: unknown, host: unknown, board: unknown):
      Promise<{ id: string; token: string; features: string[] } | { error: string; status: number }> {
    if (typeof board !== "string" || !BOARD_FINGERPRINT.test(board)) {
      return { error: "Unknown board.", status: 400 };
    }
    const client = await this.clientKey(ip);
    const now = Date.now();
    const sql = this.ctx.storage.sql;
    sql.exec("DELETE FROM creations WHERE at < ?", now - CREATE_WINDOW_MS);
    const recent = sql.exec<{ n: number }>(
      "SELECT COUNT(*) AS n FROM creations WHERE client = ?", client).one().n;
    if (recent >= limit(this.env.CREATE_LIMIT, CREATE_LIMIT)) {
      return { error: "Too many lobbies created from your network; wait a few minutes.", status: 429 };
    }
    const mine = sql.exec<{ n: number }>(
      "SELECT COUNT(*) AS n FROM lobbies WHERE client = ?", client).one().n;
    if (mine >= limit(this.env.LOBBIES_PER_CLIENT, MAX_LOBBIES_PER_CLIENT)) {
      return { error: "Your network already has the maximum number of open lobbies.", status: 429 };
    }
    const total = sql.exec<{ n: number }>("SELECT COUNT(*) AS n FROM lobbies").one().n;
    if (total >= MAX_LOBBIES) {
      return { error: "The lobby list is full; try again later.", status: 503 };
    }
    const id = randomString(12, "abcdefghijklmnopqrstuvwxyz0123456789");
    const token = randomString(48, "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
    await this.env.LOBBY.getByName(id).init(id, await sha256Hex(token));
    sql.exec(
      `INSERT INTO lobbies (id, name, host, board, created, client)
       VALUES (?, ?, ?, ?, ?, ?)`,
      id, cleanName(name, "Timezone Dominator"), cleanName(host, "Host"), board, now, client);
    sql.exec("INSERT INTO creations (client, at) VALUES (?, ?)", client, now);
    await this.ensureAlarm();
    return { id, token, features: FEATURES };
  }

  /** The host connected: list the lobby. */
  async claim(id: string): Promise<boolean> {
    const changed = this.ctx.storage.sql.exec(
      "UPDATE lobbies SET claimed = 1 WHERE id = ?", id).rowsWritten > 0;
    if (changed) this.broadcast();
    return changed;
  }

  async update(id: string, players: number, started: boolean): Promise<void> {
    const seats = Math.max(1, Math.min(MAX_GUESTS + 1, Math.floor(players)));
    const changed = this.ctx.storage.sql.exec(
      "UPDATE lobbies SET players = ?, started = ? WHERE id = ? AND claimed = 1",
      seats, started ? 1 : 0, id).rowsWritten > 0;
    if (changed) this.broadcast();
  }

  async remove(id: string): Promise<void> {
    const changed = this.ctx.storage.sql.exec(
      "DELETE FROM lobbies WHERE id = ?", id).rowsWritten > 0;
    if (changed) this.broadcast();
  }

  private async ensureAlarm(): Promise<void> {
    if ((await this.ctx.storage.getAlarm()) === null) {
      await this.ctx.storage.setAlarm(Date.now() + SWEEP_MS);
    }
  }

  /** Drop lobbies whose host never arrived, is gone, or has played too long.
   *  The host-gone check covers a lobby object that lost its sockets without
   *  a close event, e.g. across a deployment. */
  async alarm(): Promise<void> {
    const now = Date.now();
    const sql = this.ctx.storage.sql;
    const rows = sql.exec<{ id: string; claimed: number; created: number }>(
      "SELECT id, claimed, created FROM lobbies").toArray();
    const stale: string[] = [];
    for (const row of rows) {
      if (now - row.created > MAX_AGE_MS ||
          (row.claimed === 0 && now - row.created > UNCLAIMED_MS)) {
        stale.push(row.id);
      } else if (row.claimed === 1 && !(await this.env.LOBBY.getByName(row.id).hostConnected())) {
        stale.push(row.id);
      }
    }
    for (const id of stale) {
      sql.exec("DELETE FROM lobbies WHERE id = ?", id);
      await this.env.LOBBY.getByName(id).retire("The lobby expired.");
    }
    sql.exec("DELETE FROM creations WHERE at < ?", now - CREATE_WINDOW_MS);
    if (stale.length) this.broadcast();
    // Keep sweeping while anything is stored, so a hashed address in
    // `creations` never outlives its window by more than one sweep.
    const left = sql.exec<{ n: number }>(
      "SELECT (SELECT COUNT(*) FROM lobbies) + (SELECT COUNT(*) FROM creations) AS n").one().n;
    if (left > 0) await this.ctx.storage.setAlarm(Date.now() + SWEEP_MS);
  }
}

// ---------------------------------------------------------------------------
// Lobby: relays one game between its host and up to five players
// ---------------------------------------------------------------------------
export class Lobby extends DurableObject<Env> {
  /** Per-player message counts for the rate cap. In memory: an evicted
   *  object simply starts a fresh window, which only errs on the lenient side. */
  private rates = new Map<number, { start: number; count: number }>();
  private chatRates = new Map<number, { start: number; count: number }>();
  private bytes: number | null = null;
  private unflushed = 0;
  private chatBytes = 0;

  async init(id: string, tokenHash: string): Promise<void> {
    await this.ctx.storage.put({ id, tokenHash, nextPeer: 1, bytes: 0 });
  }

  /** Disconnect a misbehaving player and tell the host straight away,
   *  without waiting for the player's side of the close handshake. */
  private kick(ws: WebSocket, peer: number, code: number, reason: string): void {
    safeClose(ws, code, reason);
    this.forget(peer);
    this.host()?.send(JSON.stringify({ relay: "close", peer }));
  }

  private forget(peer: number): void {
    this.rates.delete(peer);
    this.chatRates.delete(peer);
  }

  /** Messages from `peer` in the current window of `rates`, this one included. */
  private count(rates: Map<number, { start: number; count: number }>, peer: number,
                windowMs: number): number {
    const now = Date.now();
    const rate = rates.get(peer);
    if (rate === undefined || now - rate.start > windowMs) {
      rates.set(peer, { start: now, count: 1 });
      return 1;
    }
    rate.count += 1;
    return rate.count;
  }

  private overRate(peer: number): boolean {
    return this.count(this.rates, peer, GUEST_WINDOW_MS) >
      limit(this.env.GUEST_RATE, GUEST_MESSAGES_PER_WINDOW);
  }

  /** Count chat against its own in-memory budget; false once spent. */
  private spendChat(size: number): boolean {
    this.chatBytes += size;
    return this.chatBytes <= limit(this.env.CHAT_BUDGET, CHAT_BYTE_BUDGET);
  }

  /** Count forwarded bytes against the lobby's budget; false once spent.
   *  Persisted every BUDGET_FLUSH_EVERY messages to keep storage writes low. */
  private async spend(size: number): Promise<boolean> {
    if (this.bytes === null) this.bytes = (await this.ctx.storage.get<number>("bytes")) ?? 0;
    this.bytes += size;
    this.unflushed += 1;
    if (this.unflushed >= BUDGET_FLUSH_EVERY) {
      this.unflushed = 0;
      await this.ctx.storage.put("bytes", this.bytes);
    }
    return this.bytes <= LOBBY_BYTE_BUDGET;
  }

  async hostConnected(): Promise<boolean> {
    return this.host() !== null;
  }

  /** Close everything and forget the lobby; a late join then gets 404. */
  async retire(reason: string): Promise<void> {
    for (const ws of this.ctx.getWebSockets()) safeClose(ws, 1001, reason);
    await this.ctx.storage.deleteAll();
  }

  private host(): WebSocket | null {
    return this.ctx.getWebSockets("host").find(isOpen) ?? null;
  }

  private guests(): WebSocket[] {
    return this.ctx.getWebSockets("guest").filter(isOpen);
  }

  private guest(peer: number): WebSocket | null {
    return this.ctx.getWebSockets(`peer:${peer}`).find(isOpen) ?? null;
  }

  async fetch(request: Request): Promise<Response> {
    const id = await this.ctx.storage.get<string>("id");
    if (!id) return new Response("That lobby no longer exists.", { status: 404 });
    const role = new URL(request.url).pathname.endsWith("/host") ? "host" : "join";
    const [client, server] = Object.values(new WebSocketPair());

    if (role === "host") {
      const auth = request.headers.get("Authorization") ?? "";
      const token = auth.startsWith("Bearer ") ? auth.slice(7) : "";
      const expected = await this.ctx.storage.get<string>("tokenHash");
      if (!token || (await sha256Hex(token)) !== expected) {
        return new Response("Not the host of this lobby.", { status: 403 });
      }
      if (this.host() !== null) {
        return new Response("This lobby already has a host.", { status: 409 });
      }
      this.ctx.acceptWebSocket(server, ["host"]);
      server.serializeAttachment({ role: "host" } satisfies Attachment);
      await this.env.DIRECTORY.getByName("directory").claim(id);
      return new Response(null, { status: 101, webSocket: client });
    }

    const host = this.host();
    if (host === null) return new Response("That game has ended.", { status: 410 });
    if (this.guests().length >= MAX_GUESTS) {
      return new Response("That lobby is full.", { status: 409 });
    }
    const peer = (await this.ctx.storage.get<number>("nextPeer")) ?? 1;
    await this.ctx.storage.put("nextPeer", peer + 1);
    this.ctx.acceptWebSocket(server, ["guest", `peer:${peer}`]);
    server.serializeAttachment({ role: "guest", peer } satisfies Attachment);
    host.send(JSON.stringify({ relay: "open", peer }));
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const who = ws.deserializeAttachment() as Attachment | null;
    if (who === null) return;
    if (who.role === "guest") {
      if (typeof message !== "string" || message.length > MAX_GUEST_MESSAGE_CHARS) {
        this.kick(ws, who.peer, 1009, "Message too large.");
        return;
      }
      const type = gameMessageType(message, GUEST_TYPES);
      if (type === null) {
        this.kick(ws, who.peer, 1008, "Only Timezone Dominator messages are relayed.");
        return;
      }
      if (type === "chat") {
        if (message.length > MAX_GUEST_CHAT_CHARS) {
          this.kick(ws, who.peer, 1009, "Message too large.");
          return;
        }
        const sent = this.count(this.chatRates, who.peer, CHAT_WINDOW_MS);
        if (sent > CHAT_HARD_PER_WINDOW) {
          this.kick(ws, who.peer, 1008, "Too many messages.");
          return;
        }
        if (sent > limit(this.env.CHAT_RATE, CHAT_SOFT_PER_WINDOW) || !this.spendChat(message.length)) {
          return;  // dropped: the game must not suffer for chat
        }
        this.host()?.send(JSON.stringify({ relay: "data", peer: who.peer, data: message }));
        return;
      }
      if (this.overRate(who.peer)) {
        this.kick(ws, who.peer, 1008, "Too many messages.");
        return;
      }
      if (!(await this.spend(message.length))) {
        await this.retire("This game used up its message budget.");
        return;
      }
      this.host()?.send(JSON.stringify({ relay: "data", peer: who.peer, data: message }));
      return;
    }
    if (typeof message !== "string" || message.length > MAX_HOST_FRAME_CHARS) return;
    let frame: { relay?: unknown; peer?: unknown; data?: unknown; players?: unknown; started?: unknown };
    try {
      frame = JSON.parse(message);
    } catch {
      return;
    }
    if (frame.relay === "send" && typeof frame.peer === "number" &&
        typeof frame.data === "string" && frame.data.length <= MAX_MESSAGE_CHARS) {
      const type = gameMessageType(frame.data, HOST_TYPES);
      if (type === null) return;
      if (type === "chat") {
        if (this.spendChat(frame.data.length)) this.guest(frame.peer)?.send(frame.data);
        return;
      }
      if (!(await this.spend(frame.data.length))) {
        await this.retire("This game used up its message budget.");
        return;
      }
      this.guest(frame.peer)?.send(frame.data);
    } else if (frame.relay === "close" && typeof frame.peer === "number") {
      const target = this.guest(frame.peer);
      if (target !== null) {
        safeClose(target, 1000, "Closed by the host.");
        this.forget(frame.peer);
      }
    } else if (frame.relay === "info" && typeof frame.players === "number") {
      const id = await this.ctx.storage.get<string>("id");
      if (id) {
        await this.env.DIRECTORY.getByName("directory")
          .update(id, frame.players, frame.started === true);
      }
    }
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    const who = ws.deserializeAttachment() as Attachment | null;
    safeClose(ws, 1000, "Bye.");
    if (who?.role === "guest") {
      this.forget(who.peer);
      this.host()?.send(JSON.stringify({ relay: "close", peer: who.peer }));
      return;
    }
    if (who?.role === "host") {
      const id = await this.ctx.storage.get<string>("id");
      if (id) await this.env.DIRECTORY.getByName("directory").remove(id);
      await this.retire("The host closed the game.");
    }
  }
}
