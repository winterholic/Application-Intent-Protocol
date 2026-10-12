import { test } from "node:test";
import assert from "node:assert/strict";
import { connectEventWebSocket } from "./websocket.ts";

class FakeWebSocket extends EventTarget {
  static instances: FakeWebSocket[] = [];
  readonly sent: string[] = [];
  readonly protocols: string[];
  readonly url: string;
  readyState = 0;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;

  constructor(url: string, protocols?: string | string[]) {
    super();
    this.url = url;
    this.protocols = typeof protocols === "string" ? [protocols] : protocols ?? [];
    FakeWebSocket.instances.push(this);
  }

  send(value: string): void { this.sent.push(value); }
  close(): void { this.readyState = 3; }
  open(): void { this.readyState = 1; this.dispatchEvent(new Event("open")); }
  message(data: string): void { this.dispatchEvent(new MessageEvent("message", { data })); }
}

const options = {
  url: "https://api.example/events/socket",
  token: "secret-session-token",
  sessionExpiresAtMs: Date.now() + 60_000,
  subscription: { stream: "notifications", after: "41", contractFingerprint: "a".repeat(64) },
  WebSocket: FakeWebSocket as unknown as typeof WebSocket,
};

test("browser websocket authenticates in the first frame and resumes from the explicit cursor", () => {
  FakeWebSocket.instances = [];
  const received: unknown[] = [];
  const mutableSubscription = { ...options.subscription };
  const connection = connectEventWebSocket({ ...options, subscription: mutableSubscription, onEvent: event => received.push(event) });
  mutableSubscription.stream = "changed-after-connect";
  const socket = FakeWebSocket.instances[0]!;

  assert.equal(socket.url, "wss://api.example/events/socket");
  assert.deepEqual(socket.protocols, []);
  assert.equal(socket.url.includes(options.token), false);
  socket.open();
  assert.deepEqual(JSON.parse(socket.sent[0]!), { type: "auth", token: options.token });
  assert.equal(socket.sent.some(frame => frame.includes(options.subscription.contractFingerprint)), false);

  socket.message(JSON.stringify({ type: "authenticated" }));
  assert.deepEqual(JSON.parse(socket.sent[1]!), {
    type: "subscribe", ...options.subscription,
  });
  socket.message(JSON.stringify({ type: "event", cursor: "42", payload: { message: "hello" } }));
  assert.deepEqual(received, [{ cursor: "42", payload: { message: "hello" } }]);
  assert.equal(connection.lastCursor, "42");
  connection.close();
});

test("browser websocket reports expired sessions and leaves reconnect cursor to the caller", () => {
  FakeWebSocket.instances = [];
  const stale: string[] = [];
  const connection = connectEventWebSocket({ ...options, onSessionStale: code => stale.push(code) });
  const socket = FakeWebSocket.instances[0]!;
  socket.open();
  socket.message(JSON.stringify({ type: "error", code: "TOKEN_EXPIRED" }));
  assert.deepEqual(stale, ["TOKEN_EXPIRED"]);
  assert.equal(socket.sent.length, 1);
  assert.equal(connection.lastCursor, options.subscription.after);
  assert.equal(socket.readyState, 3);
  connection.close();
  assert.equal(FakeWebSocket.instances.length, 1, "the helper does not reconnect implicitly");
});

test("browser websocket abort closes the socket and rejects stale sessions before connecting", () => {
  FakeWebSocket.instances = [];
  const controller = new AbortController();
  const connection = connectEventWebSocket({ ...options, signal: controller.signal });
  const socket = FakeWebSocket.instances[0]!;
  controller.abort();
  assert.equal(socket.readyState, 3);
  connection.close();

  assert.throws(() => connectEventWebSocket({
    ...options, sessionExpiresAtMs: Date.now() - 1,
  }), (error: any) => error.code === "TOKEN_EXPIRED");
  assert.equal(FakeWebSocket.instances.length, 1);
});

test("browser websocket refuses credential-bearing query strings", () => {
  FakeWebSocket.instances = [];
  assert.throws(() => connectEventWebSocket({ ...options, url: `${options.url}?token=${options.token}` }), (error: any) => error.code === "BAD_VALUE");
  assert.equal(FakeWebSocket.instances.length, 0);
});
