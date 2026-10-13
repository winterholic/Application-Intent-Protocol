import { failure } from "./request-id.ts";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const MAX_OBJECT_BYTES = 5 * 1024 * 1024 * 1024;
const MAX_PAYLOAD_BYTES = 512 << 10;
const MAX_DLQ_ITEMS = 50;
const MAX_DLQ_PAGE_BYTES = 1 << 20;
const MAX_SSE_LINE_BYTES = 64 * 1024;
const MAX_SSE_EVENT_BYTES = 256 * 1024;
const MAX_UPLOAD_ID_LENGTH = 2048;
const QUEUE = /^[A-Za-z0-9_.-]{1,128}$/;
const MESSAGE_STATES = new Set(["READY", "CLAIMED", "ACKED", "DEAD"]);

export type ManagedSend = (route: string, body: unknown) => Promise<unknown>;

export type ObjectMethod = "PUT" | "GET" | "HEAD" | "DELETE";
export type SignedObject = Readonly<{
  id: string;
  method: ObjectMethod;
  url: string;
  headers: Readonly<Record<string, string>>;
  expiresIn: number;
}>;
export type MultipartUpload = Readonly<{ id: string; uploadId: string }>;
export type MultipartPart = Readonly<{ number: number; etag: string }>;
export type SignedPart = Readonly<{
  id: string;
  uploadId: string;
  partNumber: number;
  size: number;
  method: "PUT";
  url: string;
  headers: Readonly<Record<string, string>>;
  expiresIn: number;
}>;
export type Message = Readonly<{
  id: string;
  owner: string;
  queue: string;
  payload: unknown;
  state: "READY" | "CLAIMED" | "ACKED" | "DEAD";
  deliveries: number;
  maxDeliveries: number;
  lastError: string | null;
}>;
export type MessageLease = Readonly<{
  id: string;
  owner: string;
  queue: string;
  payload: unknown;
  deliveries: number;
  maxDeliveries: number;
  epoch: number;
  token: string;
  worker: string;
  leaseUntil: string;
}>;
export type DeadLetter = Readonly<{
  id: string;
  owner: string;
  queue: string;
  payload: unknown;
  deliveries: number;
  maxDeliveries: number;
  lastError: string | null;
  createdAt: string;
}>;
export type DeadLetterPage = Readonly<{ items: readonly DeadLetter[]; nextCursor: string | null }>;
export type SseEvent = Readonly<{ event: string; data: string; id?: string; retryMs?: number }>;

export type InfrastructureClient = Readonly<{
  objects: Readonly<{
    sign(input: { readonly method: ObjectMethod; readonly size?: number; readonly contentType?: string; readonly id?: string }): Promise<SignedObject>;
    multipartStart(input: { readonly contentType: string }): Promise<MultipartUpload>;
    part(input: { readonly id: string; readonly uploadId: string; readonly partNumber: number; readonly size: number }): Promise<SignedPart>;
    complete(input: { readonly id: string; readonly uploadId: string; readonly parts: readonly MultipartPart[] }): Promise<Readonly<{ id: string; completed: true }>>;
    abort(input: { readonly id: string; readonly uploadId: string }): Promise<Readonly<{ id: string; aborted: true }>>;
  }>;
  messages: Readonly<{
    enqueue(input: { readonly queue: string; readonly key: string; readonly payload: unknown; readonly delayMs?: number; readonly ttlMs?: number; readonly maxDeliveries?: number }): Promise<Message>;
    claim(input: { readonly queue: string; readonly worker: string; readonly leaseMs?: number }): Promise<MessageLease | null>;
    ack(input: { readonly queue: string; readonly lease: MessageLease }): Promise<boolean>;
    nack(input: { readonly queue: string; readonly lease: MessageLease; readonly error?: string }): Promise<boolean>;
    dead(input: { readonly queue: string; readonly limit?: number; readonly cursor?: string }): Promise<DeadLetterPage>;
    redrive(input: { readonly queue: string; readonly id: string; readonly maxDeliveries?: number }): Promise<boolean>;
  }>;
}>;

function error(code: string, message: string): Error & { code: string } {
  return Object.assign(new Error(message), { code });
}

function badValue(): never {
  throw error("BAD_VALUE", "인프라 요청 값이 잘못되었습니다");
}

function protocolError(): never {
  throw error("PROTOCOL_ERROR", "인프라 응답 형식이 잘못되었습니다");
}

/** Parse a `text/event-stream` response incrementally with bounded line and event buffers. */
export async function* readSse(response: Response, signal?: AbortSignal): AsyncGenerator<SseEvent, void, unknown> {
  if (!(response instanceof Response) || !response.ok || !response.body || !/^text\/event-stream(?:\s*;|$)/i.test(response.headers.get("content-type") ?? "")) protocolError();
  if (signal?.aborted) throw signal.reason ?? new DOMException("SSE 읽기가 취소되었습니다", "AbortError");
  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let pending = "";
  let data: string[] = [];
  let eventName = "";
  let lastEventId: string | undefined;
  let retryMs: number | undefined;
  let eventBytes = 0;

  const dispatch = (): SseEvent | undefined => {
    if (data.length === 0) {
      eventName = "";
      retryMs = undefined;
      return undefined;
    }
    const event: SseEvent = {
      event: eventName || "message",
      data: data.join("\n"),
      ...(lastEventId === undefined ? {} : { id: lastEventId }),
      ...(retryMs === undefined ? {} : { retryMs }),
    };
    data = [];
    eventName = "";
    retryMs = undefined;
    eventBytes = 0;
    return Object.freeze(event);
  };

  const parseLine = (rawLine: string): SseEvent | undefined => {
    const line = rawLine.endsWith("\r") ? rawLine.slice(0, -1) : rawLine;
    if (new TextEncoder().encode(line).byteLength > MAX_SSE_LINE_BYTES) protocolError();
    if (line === "") return dispatch();
    if (line.startsWith(":")) return undefined;
    const colon = line.indexOf(":");
    const field = colon < 0 ? line : line.slice(0, colon);
    const value = colon < 0 ? "" : line.slice(colon + 1).replace(/^ /, "");
    switch (field) {
      case "data": {
        eventBytes += new TextEncoder().encode(value).byteLength + (data.length === 0 ? 0 : 1);
        if (eventBytes > MAX_SSE_EVENT_BYTES) protocolError();
        data.push(value);
        break;
      }
      case "event": eventName = value; break;
      case "id":
        if (!value.includes("\0")) lastEventId = value;
        break;
      case "retry":
        if (/^\d+$/.test(value)) {
          const valueMs = Number(value);
          if (Number.isSafeInteger(valueMs) && valueMs <= 2_147_483_647) retryMs = valueMs;
        }
        break;
    }
    return undefined;
  };

  const drainLines = (final = false): SseEvent[] => {
    const events: SseEvent[] = [];
    while (true) {
      const lf = pending.indexOf("\n");
      const cr = pending.indexOf("\r");
      const newline = lf < 0 ? cr : cr < 0 ? lf : Math.min(lf, cr);
      if (newline < 0 || (!final && pending[newline] === "\r" && newline === pending.length - 1)) break;
      const line = pending.slice(0, newline);
      const delimiterWidth = pending[newline] === "\r" && pending[newline + 1] === "\n" ? 2 : 1;
      pending = pending.slice(newline + delimiterWidth);
      const event = parseLine(line);
      if (event) events.push(event);
    }
    if (final && pending.length > 0) {
      const event = parseLine(pending);
      if (event) events.push(event);
      pending = "";
    }
    return events;
  };

  const onAbort = () => { void reader.cancel(signal?.reason).catch(() => {}); };
  signal?.addEventListener("abort", onAbort, { once: true });
  try {
    while (true) {
      if (signal?.aborted) throw signal.reason ?? new DOMException("SSE 읽기가 취소되었습니다", "AbortError");
      const { value, done } = await reader.read();
      if (signal?.aborted) throw signal.reason ?? new DOMException("SSE 읽기가 취소되었습니다", "AbortError");
      if (done) break;
      for (let offset = 0; offset < value.byteLength; offset += 16 * 1024) {
        let decoded: string;
        try {
          decoded = decoder.decode(value.subarray(offset, Math.min(value.byteLength, offset + 16 * 1024)), { stream: true });
        } catch {
          protocolError();
        }
        pending += decoded!;
        for (const event of drainLines()) yield event;
        if (new TextEncoder().encode(pending).byteLength > MAX_SSE_LINE_BYTES) protocolError();
      }
    }
    let tail: string;
    try {
      tail = decoder.decode();
    } catch {
      protocolError();
    }
    pending += tail!;
    for (const event of drainLines(true)) yield event;
    const finalEvent = dispatch();
    if (finalEvent) yield finalEvent;
  } finally {
    signal?.removeEventListener("abort", onAbort);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

function plainRecord(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function isUuid(value: unknown): value is string {
  return typeof value === "string" && UUID.test(value);
}

function isBoundedText(value: unknown, max: number, allowEmpty = false): value is string {
  return typeof value === "string" && new TextEncoder().encode(value).byteLength <= max && (allowEmpty || value.length > 0) && !/[\u0000-\u001f\u007f-\u009f]/u.test(value);
}

function validContentType(value: unknown): value is string {
  return typeof value === "string" && value.length <= 127 && /^[A-Za-z0-9.+-]+\/[A-Za-z0-9.+-]+$/.test(value);
}

function validateObjectUrl(value: unknown): asserts value is string {
  if (typeof value !== "string" || value.length > 8192 || value.includes("\r") || value.includes("\n")) protocolError();
  // `//host` and `/\host` are parsed as another origin by URL, so a "relative" value must be a plain absolute path.
  if (value.includes("\\") || value.startsWith("//") || !(value.startsWith("/") || /^https:\/\//i.test(value))) protocolError();
  try {
    const url = new URL(value, "https://managed.invalid");
    if (url.protocol !== "https:" && !(value.startsWith("/") && url.origin === "https://managed.invalid")) protocolError();
    if (url.username || url.password) protocolError();
  } catch {
    protocolError();
  }
}

const FORBIDDEN_SIGNED_HEADERS = new Set(["authorization", "proxy-authorization", "cookie"]);

function validateHeaders(value: unknown): Readonly<Record<string, string>> {
  if (value === undefined) return Object.freeze({});
  if (!plainRecord(value) || Object.keys(value).length > 32) protocolError();
  const headers: Record<string, string> = {};
  for (const [name, header] of Object.entries(value)) {
    if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]{1,128}$/.test(name) || typeof header !== "string" || header.length > 4096 || /[\r\n\0]/.test(header)) protocolError();
    // A signed-request header list may carry content metadata, never credentials.
    if (FORBIDDEN_SIGNED_HEADERS.has(name.toLowerCase())) protocolError();
    headers[name.toLowerCase()] = header;
  }
  return Object.freeze(headers);
}

function deepFreeze<T>(value: T): T {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const child of Object.values(value as Record<string, unknown>)) deepFreeze(child);
  }
  return value;
}

function snapshotJson(value: unknown, maxBytes: number): unknown {
  let encoded: string;
  try {
    encoded = JSON.stringify(value);
  } catch {
    return badValue();
  }
  if (encoded === undefined || new TextEncoder().encode(encoded).byteLength > maxBytes) return badValue();
  try {
    return JSON.parse(encoded);
  } catch {
    return badValue();
  }
}

function validateQueue(value: unknown): asserts value is string {
  if (typeof value !== "string" || !QUEUE.test(value)) badValue();
}

function validateLease(value: unknown, queue: string): MessageLease {
  if (!plainRecord(value) || !isUuid(value.id) || !isBoundedText(value.owner, 512) || value.queue !== queue ||
      !isBoundedText(value.queue, 128) || !Number.isSafeInteger(value.deliveries) || (value.deliveries as number) < 1 ||
      !Number.isSafeInteger(value.max_deliveries) || (value.max_deliveries as number) < 1 || (value.max_deliveries as number) > 100 ||
      !Number.isSafeInteger(value.epoch) || (value.epoch as number) < 1 || !isUuid(value.token) || !isBoundedText(value.worker, 256) ||
      !isBoundedText(value.lease_until, 64) || !Number.isFinite(Date.parse(value.lease_until))) protocolError();
  const payload = snapshotJson(value.payload, MAX_PAYLOAD_BYTES);
  return deepFreeze({
    id: value.id.toLowerCase(), owner: value.owner, queue: value.queue, payload,
    deliveries: value.deliveries as number, maxDeliveries: value.max_deliveries as number,
    epoch: value.epoch as number, token: value.token.toLowerCase(), worker: value.worker, leaseUntil: value.lease_until,
  });
}

function leaseToWire(lease: MessageLease): Record<string, unknown> {
  return {
    id: lease.id,
    owner: lease.owner,
    queue: lease.queue,
    payload: snapshotJson(lease.payload, MAX_PAYLOAD_BYTES),
    deliveries: lease.deliveries,
    max_deliveries: lease.maxDeliveries,
    epoch: lease.epoch,
    token: lease.token,
    worker: lease.worker,
    lease_until: lease.leaseUntil,
  };
}

function normalizeMessage(value: unknown, queue: string): Message {
  if (!plainRecord(value) || !isUuid(value.id) || !isBoundedText(value.owner, 512) || value.queue !== queue ||
      !MESSAGE_STATES.has(value.state as string) || !Number.isSafeInteger(value.deliveries) || (value.deliveries as number) < 0 ||
      !Number.isSafeInteger(value.max_deliveries) || (value.max_deliveries as number) < 1 || (value.max_deliveries as number) > 100 ||
      !(value.last_error === null || isBoundedText(value.last_error, 512))) protocolError();
  const payload = snapshotJson(value.payload, MAX_PAYLOAD_BYTES);
  return deepFreeze({
    id: value.id.toLowerCase(), owner: value.owner, queue, payload, state: value.state as Message["state"],
    deliveries: value.deliveries as number, maxDeliveries: value.max_deliveries as number,
    lastError: value.last_error as string | null,
  });
}

function normalizeDeadLetter(value: unknown, queue: string): DeadLetter {
  if (!plainRecord(value) || !isUuid(value.id) || !isBoundedText(value.owner, 512) || value.queue !== queue ||
      !Number.isSafeInteger(value.deliveries) || (value.deliveries as number) < 1 || !Number.isSafeInteger(value.max_deliveries) ||
      (value.max_deliveries as number) < 1 || (value.max_deliveries as number) > 100 ||
      !(value.last_error === null || isBoundedText(value.last_error, 512)) || !isBoundedText(value.created_at, 64) || !Number.isFinite(Date.parse(value.created_at))) protocolError();
  return deepFreeze({
    id: value.id.toLowerCase(), owner: value.owner, queue, payload: snapshotJson(value.payload, MAX_PAYLOAD_BYTES),
    deliveries: value.deliveries as number, maxDeliveries: value.max_deliveries as number,
    lastError: value.last_error as string | null, createdAt: value.created_at,
  });
}

/** Create object-storage and messaging helpers over the authenticated managed transport. */
export function createInfrastructureClient(send: ManagedSend, fingerprint: string): InfrastructureClient {
  if (typeof send !== "function" || typeof fingerprint !== "string" || !/^[a-f0-9]{64}$/.test(fingerprint)) {
    throw error("PROTOCOL_ERROR", "인프라 클라이언트 계약 지문 또는 transport가 잘못되었습니다");
  }

  async function call(route: string, body: unknown): Promise<Record<string, unknown>> {
    const value = await send(route, body);
    if (!plainRecord(value)) protocolError();
    if (value.ok === false && typeof value.code === "string") {
      throw failure(value.code, typeof value.msg === "string" ? value.msg : value.code, value);
    }
    if (value.ok !== true) protocolError();
    if (value.contractFingerprint !== fingerprint) throw failure("CONTRACT_MISMATCH", "서버와 생성 계약이 다릅니다", value);
    return value;
  }

  const objects = {
    async sign(input: { readonly method: ObjectMethod; readonly size?: number; readonly contentType?: string; readonly id?: string }): Promise<SignedObject> {
      if (!input || !["PUT", "GET", "HEAD", "DELETE"].includes(input.method)) badValue();
      if (input.method === "PUT") {
        if (!Number.isSafeInteger(input.size) || (input.size as number) < 0 || (input.size as number) > MAX_OBJECT_BYTES || !validContentType(input.contentType)) badValue();
        if (input.id !== undefined && !isUuid(input.id)) badValue();
      } else if (!isUuid(input.id) || input.size !== undefined || input.contentType !== undefined) badValue();
      const body = {
        method: input.method,
        ...(input.size === undefined ? {} : { size: input.size }),
        ...(input.contentType === undefined ? {} : { contentType: input.contentType }),
        ...(input.id === undefined ? {} : { id: input.id.toLowerCase() }),
      };
      const response = await call("/objects/sign", body);
      if (!isUuid(response.id) || response.id.toLowerCase() !== (input.id?.toLowerCase() ?? response.id.toLowerCase()) || response.method !== input.method ||
          !Number.isSafeInteger(response.expiresIn) || (response.expiresIn as number) < 1 || (response.expiresIn as number) > 604800) protocolError();
      validateObjectUrl(response.url);
      const headers = validateHeaders(response.headers);
      if (input.method === "PUT" && (headers["content-length"] !== String(input.size) || headers["content-type"] !== input.contentType)) protocolError();
      return deepFreeze({ id: response.id.toLowerCase(), method: input.method, url: response.url, headers, expiresIn: response.expiresIn as number });
    },

    async multipartStart(input: { readonly contentType: string }): Promise<MultipartUpload> {
      if (!input || !validContentType(input.contentType)) badValue();
      const response = await call("/objects/multipart/start", { contentType: input.contentType });
      if (!isUuid(response.id) || !isBoundedText(response.uploadId, MAX_UPLOAD_ID_LENGTH)) protocolError();
      return deepFreeze({ id: response.id.toLowerCase(), uploadId: response.uploadId });
    },

    async part(input: { readonly id: string; readonly uploadId: string; readonly partNumber: number; readonly size: number }): Promise<SignedPart> {
      if (!input || !isUuid(input.id) || !isBoundedText(input.uploadId, MAX_UPLOAD_ID_LENGTH) ||
          !Number.isSafeInteger(input.partNumber) || input.partNumber < 1 || input.partNumber > 10_000 ||
          !Number.isSafeInteger(input.size) || input.size < 1 || input.size > MAX_OBJECT_BYTES) badValue();
      const id = input.id.toLowerCase();
      const body = { id, uploadId: input.uploadId, partNumber: input.partNumber, size: input.size };
      const response = await call("/objects/multipart/part", body);
      if (!isUuid(response.id) || response.id.toLowerCase() !== id || response.partNumber !== input.partNumber || response.method !== "PUT") protocolError();
      validateObjectUrl(response.url);
      const headers = validateHeaders(response.headers);
      if (headers["content-length"] !== String(input.size)) protocolError();
      const expiresIn = response.expiresIn;
      if (!Number.isSafeInteger(expiresIn) || (expiresIn as number) < 1 || (expiresIn as number) > 604800) protocolError();
      return deepFreeze({ id, uploadId: input.uploadId, partNumber: input.partNumber, size: input.size, method: "PUT", url: response.url, headers, expiresIn: expiresIn as number });
    },

    async complete(input: { readonly id: string; readonly uploadId: string; readonly parts: readonly MultipartPart[] }): Promise<Readonly<{ id: string; completed: true }>> {
      if (!input || !isUuid(input.id) || !isBoundedText(input.uploadId, MAX_UPLOAD_ID_LENGTH) || !Array.isArray(input.parts) || input.parts.length < 1 || input.parts.length > 10_000) badValue();
      let previous = 0;
      const parts = input.parts.map(part => {
        if (!part || !Number.isSafeInteger(part.number) || part.number <= previous || part.number > 10_000 || !isBoundedText(part.etag, 256)) badValue();
        previous = part.number;
        return { number: part.number, etag: part.etag };
      });
      const id = input.id.toLowerCase();
      const response = await call("/objects/multipart/complete", { id, uploadId: input.uploadId, parts });
      if (!isUuid(response.id) || response.id.toLowerCase() !== id || response.completed !== true) protocolError();
      return deepFreeze({ id, completed: true as const });
    },

    async abort(input: { readonly id: string; readonly uploadId: string }): Promise<Readonly<{ id: string; aborted: true }>> {
      if (!input || !isUuid(input.id) || !isBoundedText(input.uploadId, MAX_UPLOAD_ID_LENGTH)) badValue();
      const id = input.id.toLowerCase();
      const response = await call("/objects/multipart/abort", { id, uploadId: input.uploadId });
      if (!isUuid(response.id) || response.id.toLowerCase() !== id) protocolError();
      return deepFreeze({ id, aborted: true as const });
    },
  };

  const messages = {
    async enqueue(input: { readonly queue: string; readonly key: string; readonly payload: unknown; readonly delayMs?: number; readonly ttlMs?: number; readonly maxDeliveries?: number }): Promise<Message> {
      if (!input) badValue();
      validateQueue(input.queue);
      if (!isBoundedText(input.key, 256) || input.delayMs !== undefined && (!Number.isSafeInteger(input.delayMs) || input.delayMs < 0 || input.delayMs > 86_400_000) ||
          input.ttlMs !== undefined && (!Number.isSafeInteger(input.ttlMs) || input.ttlMs < 1 || input.ttlMs > 86_400_000) ||
          input.maxDeliveries !== undefined && (!Number.isSafeInteger(input.maxDeliveries) || input.maxDeliveries < 1 || input.maxDeliveries > 100)) badValue();
      const payload = snapshotJson(input.payload, MAX_PAYLOAD_BYTES);
      const body = {
        queue: input.queue, key: input.key, payload,
        ...(input.delayMs === undefined ? {} : { delayMs: input.delayMs }),
        ...(input.ttlMs === undefined ? {} : { ttlMs: input.ttlMs }),
        ...(input.maxDeliveries === undefined ? {} : { maxDeliveries: input.maxDeliveries }),
      };
      const response = await call("/messages/enqueue", body);
      return normalizeMessage(response.message, input.queue);
    },

    async claim(input: { readonly queue: string; readonly worker: string; readonly leaseMs?: number }): Promise<MessageLease | null> {
      if (!input) badValue();
      validateQueue(input.queue);
      if (!isBoundedText(input.worker, 256) || input.leaseMs !== undefined && (!Number.isSafeInteger(input.leaseMs) || input.leaseMs < 100 || input.leaseMs > 300_000)) badValue();
      const body = { queue: input.queue, worker: input.worker, ...(input.leaseMs === undefined ? {} : { leaseMs: input.leaseMs }) };
      const response = await call("/messages/claim", body);
      return response.lease === null ? null : validateLease(response.lease, input.queue);
    },

    async ack(input: { readonly queue: string; readonly lease: MessageLease }): Promise<boolean> {
      if (!input) badValue();
      validateQueue(input.queue);
      const lease = validateLease(leaseToWire(input.lease), input.queue);
      const response = await call("/messages/ack", { queue: input.queue, lease: leaseToWire(lease) });
      if (typeof response.acknowledged !== "boolean") protocolError();
      return response.acknowledged;
    },

    async nack(input: { readonly queue: string; readonly lease: MessageLease; readonly error?: string }): Promise<boolean> {
      if (!input) badValue();
      validateQueue(input.queue);
      if (input.error !== undefined && !isBoundedText(input.error, 512)) badValue();
      const lease = validateLease(leaseToWire(input.lease), input.queue);
      const response = await call("/messages/nack", { queue: input.queue, lease: leaseToWire(lease), ...(input.error === undefined ? {} : { error: input.error }) });
      if (typeof response.released !== "boolean") protocolError();
      return response.released;
    },

    async dead(input: { readonly queue: string; readonly limit?: number; readonly cursor?: string }): Promise<DeadLetterPage> {
      if (!input) badValue();
      validateQueue(input.queue);
      const limit = input.limit ?? MAX_DLQ_ITEMS;
      if (!Number.isSafeInteger(limit) || limit < 1 || limit > MAX_DLQ_ITEMS || input.cursor !== undefined && !isUuid(input.cursor)) badValue();
      const response = await call("/messages/dead", { queue: input.queue, limit, ...(input.cursor === undefined ? {} : { after: input.cursor.toLowerCase() }) });
      if (!Array.isArray(response.items) || response.items.length > limit || !(response.next_cursor === null || isBoundedText(response.next_cursor, 1024, true))) protocolError();
      const items = response.items.map(item => normalizeDeadLetter(item, input.queue));
      const bytes = new TextEncoder().encode(JSON.stringify(items)).byteLength;
      if (items.length > MAX_DLQ_ITEMS || bytes > MAX_DLQ_PAGE_BYTES) protocolError();
      return deepFreeze({ items, nextCursor: response.next_cursor as string | null });
    },

    async redrive(input: { readonly queue: string; readonly id: string; readonly maxDeliveries?: number }): Promise<boolean> {
      if (!input) badValue();
      validateQueue(input.queue);
      if (!isUuid(input.id) || input.maxDeliveries !== undefined && (!Number.isSafeInteger(input.maxDeliveries) || input.maxDeliveries < 1 || input.maxDeliveries > 100)) badValue();
      const response = await call("/messages/redrive", { queue: input.queue, id: input.id.toLowerCase(), ...(input.maxDeliveries === undefined ? {} : { maxDeliveries: input.maxDeliveries }) });
      if (typeof response.redriven !== "boolean") protocolError();
      return response.redriven;
    },
  };

  return Object.freeze({ objects: Object.freeze(objects), messages: Object.freeze(messages) });
}
