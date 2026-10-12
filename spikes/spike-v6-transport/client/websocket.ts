export type EventSubscription = Readonly<{
  stream: string;
  after: string;
  contractFingerprint: string;
}>;

export type RealtimeEvent = Readonly<{ cursor: string; payload: unknown }>;

export type BrowserEventSocketOptions = Readonly<{
  url: string;
  token: string;
  sessionExpiresAtMs: number;
  subscription: EventSubscription;
  signal?: AbortSignal;
  WebSocket?: typeof WebSocket;
  authTimeoutMs?: number;
  onEvent?: (event: RealtimeEvent) => void;
  onError?: (error: Error & { code: string }) => void;
  onSessionStale?: (code: string) => void;
}>;

export type BrowserEventSocket = Readonly<{
  socket: WebSocket;
  readonly lastCursor: string;
  close(): void;
}>;

const CURSOR = /^(0|[1-9][0-9]{0,18})$/;
const FINGERPRINT = /^[a-f0-9]{64}$/;
const MAX_TOKEN_LENGTH = 4096;
const MAX_AUTH_TIMEOUT_MS = 30_000;

function clientError(code: string, message = code): Error & { code: string } {
  return Object.assign(new Error(message), { code });
}

function websocketUrl(value: string): string {
  let url: URL;
  try { url = new URL(value); } catch { throw clientError("BAD_VALUE", "WebSocket 주소가 잘못되었습니다"); }
  if (url.protocol === "https:") url.protocol = "wss:";
  else if (url.protocol === "http:") url.protocol = "ws:";
  if (!(["ws:", "wss:"].includes(url.protocol)) || url.username || url.password || url.hash || url.search) {
    throw clientError("BAD_VALUE", "WebSocket 주소는 query, 자격 증명과 fragment 없이 ws 또는 wss여야 합니다");
  }
  return url.toString();
}

/**
 * Open one browser WebSocket using a bounded first-frame auth exchange.
 * Reconnect explicitly with `lastCursor`; no background retry can silently replay work.
 */
export function connectEventWebSocket(options: BrowserEventSocketOptions): BrowserEventSocket {
  if (!options || typeof options.token !== "string" || options.token.length === 0 || options.token.length > MAX_TOKEN_LENGTH ||
      !Number.isSafeInteger(options.sessionExpiresAtMs) || options.sessionExpiresAtMs <= Date.now() ||
      !options.subscription || typeof options.subscription.stream !== "string" || options.subscription.stream.length < 1 || options.subscription.stream.length > 128 ||
      typeof options.subscription.after !== "string" || !CURSOR.test(options.subscription.after) ||
      typeof options.subscription.contractFingerprint !== "string" || !FINGERPRINT.test(options.subscription.contractFingerprint)) {
    throw clientError(options?.sessionExpiresAtMs <= Date.now() ? "TOKEN_EXPIRED" : "BAD_VALUE", "WebSocket 인증 또는 구독 값이 잘못되었습니다");
  }
  const authTimeoutMs = options.authTimeoutMs ?? 5_000;
  if (!Number.isSafeInteger(authTimeoutMs) || authTimeoutMs < 100 || authTimeoutMs > MAX_AUTH_TIMEOUT_MS) {
    throw clientError("BAD_VALUE", "WebSocket 인증 제한 시간이 잘못되었습니다");
  }
  if (options.signal?.aborted) throw options.signal.reason ?? new DOMException("연결이 취소되었습니다", "AbortError");

  const subscription = Object.freeze({
    stream: options.subscription.stream,
    after: options.subscription.after,
    contractFingerprint: options.subscription.contractFingerprint,
  });
  const Constructor = options.WebSocket ?? globalThis.WebSocket;
  if (typeof Constructor !== "function") throw clientError("NOT_AVAILABLE", "WebSocket을 사용할 수 없습니다");
  const socket = new Constructor(websocketUrl(options.url));
  let cursor = subscription.after;
  let closed = false;
  let authenticated = false;
  let authTimer: ReturnType<typeof setTimeout> | undefined;
  let expiryTimer: ReturnType<typeof setTimeout> | undefined;

  const report = (code: string, message = code) => options.onError?.(clientError(code, message));
  const clearTimers = () => {
    if (authTimer !== undefined) clearTimeout(authTimer);
    if (expiryTimer !== undefined) clearTimeout(expiryTimer);
    authTimer = undefined;
    expiryTimer = undefined;
  };
  const removeAbort = () => options.signal?.removeEventListener("abort", onAbort);
  const close = () => {
    if (closed) return;
    closed = true;
    clearTimers();
    removeAbort();
    socket.close(1000, "client closed");
  };
  const stale = (code: string) => {
    if (closed) return;
    closed = true;
    clearTimers();
    removeAbort();
    options.onSessionStale?.(code);
    socket.close(1008, code);
  };
  const onAbort = () => close();
  options.signal?.addEventListener("abort", onAbort, { once: true });

  const expiryDelay = options.sessionExpiresAtMs - Date.now();
  const scheduleExpiry = () => {
    const remaining = options.sessionExpiresAtMs - Date.now();
    if (remaining <= 0) stale("TOKEN_EXPIRED");
    else expiryTimer = setTimeout(scheduleExpiry, Math.min(remaining, 2_147_483_647));
  };
  expiryTimer = setTimeout(scheduleExpiry, Math.min(expiryDelay, 2_147_483_647));
  authTimer = setTimeout(() => {
    if (!authenticated && !closed) {
      report("AUTH_TIMEOUT", "WebSocket 인증 ACK 제한 시간 초과");
      close();
    }
  }, authTimeoutMs);

  socket.addEventListener("open", () => {
    if (!closed) socket.send(JSON.stringify({ type: "auth", token: options.token }));
  }, { once: true });
  socket.addEventListener("message", event => {
    if (closed || typeof event.data !== "string" || event.data.length > 256 * 1024) return;
    let message: unknown;
    try { message = JSON.parse(event.data); } catch { report("PROTOCOL_ERROR", "WebSocket JSON 응답이 잘못되었습니다"); close(); return; }
    if (!message || typeof message !== "object" || Array.isArray(message)) return;
    const value = message as Record<string, unknown>;
    if (value.type === "error" && typeof value.code === "string") {
      if (["TOKEN_EXPIRED", "UNAUTHENTICATED", "DEPLOYMENT_CHANGED"].includes(value.code)) stale(value.code);
      else { report(value.code); close(); }
      return;
    }
    if (!authenticated) {
      if (value.type !== "authenticated") { report("PROTOCOL_ERROR", "인증 완료 응답이 필요합니다"); close(); return; }
      authenticated = true;
      if (authTimer !== undefined) clearTimeout(authTimer);
      authTimer = undefined;
      socket.send(JSON.stringify({ type: "subscribe", ...subscription }));
      return;
    }
    if (value.type !== "event" || typeof value.cursor !== "string" || !CURSOR.test(value.cursor) || !Object.hasOwn(value, "payload")) {
      report("PROTOCOL_ERROR", "WebSocket 이벤트 응답이 잘못되었습니다");
      close();
      return;
    }
    cursor = value.cursor;
    options.onEvent?.(Object.freeze({ cursor, payload: value.payload }));
  });
  socket.addEventListener("error", () => report("NETWORK_ERROR", "WebSocket 연결 오류"));
  socket.addEventListener("close", () => {
    closed = true;
    clearTimers();
    removeAbort();
  }, { once: true });

  return Object.freeze({ socket, get lastCursor() { return cursor; }, close });
}
