const MANAGED_PATHS = new Set(["/rag/search", "/rag/answer", "/rag/index", "/rag/delete", "/jobs/enqueue", "/jobs/status", "/jobs/cancel", "/jobs/result", "/events/read", "/files/put", "/files/get", "/files/delete", "/objects/sign", "/objects/multipart/start", "/objects/multipart/part", "/objects/multipart/complete", "/objects/multipart/abort", "/messages/enqueue", "/messages/claim", "/messages/ack", "/messages/nack", "/messages/dead", "/messages/redrive"]);
// V6 클라이언트(실험). 전송 + V5 캐시.
// 쓰기 응답을 확정하지 못하면(통신 실패, COMMIT_UNKNOWN·INTERNAL) 같은 키·같은 본문으로 다시 보낸다.
// 서버는 키 단위로 직렬화하므로 커밋됐으면 저장 결과, 진행 중이면 끝난 뒤 결과, 롤백됐으면 새 실행을 돌려준다.
// 확정 응답을 받기 전까지 그 키는 미확정으로 남고 캐시 저장도 보류된다(r7 R7-02·R7-03).
import { createCache, ScopeChanged } from "../../spike-v5-sdk/sdk/cache.ts";
import type { IdWireKind } from "../../spike-v5-sdk/sdk/generic.ts";
import { failure, rememberRequestId, requestIdOf, retryOf, type RetryGuidance } from "./request-id.ts";

export type ApplyOptions = { key?: string; dropResponse?: boolean; retries?: number };
export function isIdWire(value: unknown): value is IdWireKind {
  return value === "legacy" || value === "safe-number-v13" || value === "decimal-string-v13";
}

export function validResultId(value: unknown, wire: IdWireKind, kind: "uuid" | "bigint" = "bigint"): boolean {
  if (kind === "uuid") return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
  if (wire === "decimal-string-v13") {
    return typeof value === "string" && value.length <= 19 && /^(0|[1-9][0-9]*)$/.test(value) && BigInt(value) <= (1n << 63n) - 1n;
  }
  if (wire === "safe-number-v13") return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
  return typeof value === "number" && Number.isInteger(value) && value >= -(2 ** 63) && value <= 2 ** 63;
}

function validWriteResponse(res: any, wire?: IdWireKind, extensionWrite = false, request?: any, kinds?: Readonly<Record<string,"uuid"|"bigint">>): boolean {
  const valid = (v:unknown,res:string) => wire === undefined || validResultId(v,wire,kinds?.[res] ?? "bigint");
  const metadata = wire === undefined || (res?.replayed === undefined || typeof res.replayed === "boolean") && (res?.recovered === undefined || typeof res.recovered === "boolean");
  if (!metadata) return false;
  if (res?.ok === false) return typeof res.code === "string" && (wire === undefined || res.msg === undefined || typeof res.msg === "string");
  if (res?.ok !== true || !Array.isArray(res.tags) || !res.tags.every((tag: unknown) => typeof tag === "string")) return false;
  if (extensionWrite) return res.output !== null && typeof res.output === "object" && !Array.isArray(res.output);
  // 묶음·조합 결과: 연산마다 바뀐 id와 만든 행. 만든 행의 id는 `id` 열이 없는 resource면 null이다.
  if (res.applied !== undefined || res.created !== undefined) {
    const appliedResources = Array.isArray(request?.atomic) ? request.atomic.filter((op:any)=>typeof op?.apply==="string").map((op:any)=>op.apply.split(".")[0]) : Array(res.applied?.length ?? 0).fill(request?.compose);
    const idsOk = (ids: unknown,resource:string) => Array.isArray(ids) && ids.every(id => valid(id,resource));
    return Array.isArray(res.applied) && Array.isArray(res.created) &&
      res.applied.every((row: any,index:number) => row !== null && typeof row === "object" && idsOk(row.changed,appliedResources[index]) && idsOk(row.unchanged,appliedResources[index])) &&
      res.created.every((row: any) => row !== null && typeof row === "object" && typeof row.resource === "string" &&
        (row.id === null || valid(row.id,row.resource)));
  }
  return wire === undefined || [res.changed, res.unchanged].every(ids => Array.isArray(ids) && ids.every(id => valid(id,request?.apply?.split(".")[0])));
}

const UNSETTLED = new Set(["WIRE.COMMIT_UNKNOWN", "WIRE.INTERNAL"]);
// 실행 전 거부나 이번 시도의 rollback은 같은 키의 이전 커밋 여부를 알려 주지 않는다.
const NOT_EXECUTED = new Set(["WIRE.TOKEN_EXPIRED", "WIRE.UNAUTHENTICATED", "WIRE.BAD_REQUEST", "WIRE.PAYLOAD_TOO_LARGE", "WIRE.NOT_FOUND", "WIRE.CONTRACT_MISMATCH", "WIRE.CONTRACT_REQUIRED", "WIRE.ORIGIN_NOT_ALLOWED", "WIRE.REQUEST_TIMEOUT", "WIRE.DB_UNAVAILABLE", "WIRE.WORKER_BUSY", "WIRE.DEADLINE_EXCEEDED", "WIRE.CONFLICT"]);

export class WriteUnsettled extends Error {
  key: string;
  request: unknown;
  /** 서버가 마지막으로 답한 요청의 ID. 한 번도 응답을 받지 못했으면 없다. */
  requestId?: string;
  retry?: RetryGuidance;
  constructor(key: string, request: unknown, requestId?: string, retry?: RetryGuidance) {
    super("쓰기 결과를 아직 확정하지 못함");
    this.key = key;
    this.request = request;
    if (requestId !== undefined) this.requestId = requestId;
    if (retry !== undefined) this.retry = retry;
  }
}

export class ScopeConflict extends Error {
  name = "ScopeConflict";
}

type Pending = { request: unknown; attempts: number; outcome?: any; lastRequestId?: string; lastRetry?: RetryGuidance };
type AuthContext = { token: string | null; generation: number };

// 미확정 쓰기는 서버 검증 principal에, 캐시·진행 중 요청은 인증 세대에 귀속한다.
/** Largest response body the client will read; the server never answers with more than a few MiB. */
export const DEFAULT_MAX_RESPONSE_BYTES = 32 * 1024 * 1024;

// A reverse proxy or an auth portal answers with an HTML page, and a broken server can answer without end. Both must fail as
// a protocol error before the body is parsed, not as a SyntaxError or by exhausting memory. A missing or text/plain type is
// accepted (some stacks omit or default it); anything else that is not JSON is not an AIP response.
async function readJsonBounded(response: Response, limit: number): Promise<any> {
  const media = (response.headers.get("content-type") ?? "").split(";")[0].trim().toLowerCase();
  if (media !== "" && media !== "application/json" && media !== "text/plain" && !media.endsWith("+json")) {
    await response.body?.cancel().catch(() => {});
    throw failure("CLIENT.PROTOCOL_ERROR", "응답이 JSON이 아님");
  }
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > limit) {
    await response.body?.cancel().catch(() => {});
    throw failure("CLIENT.PROTOCOL_ERROR", "응답이 크기 상한을 넘음");
  }
  const reader = response.body?.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  while (reader) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > limit) {
      await reader.cancel().catch(() => {});
      throw failure("CLIENT.PROTOCOL_ERROR", "응답이 크기 상한을 넘음");
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  try {
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    throw failure("CLIENT.PROTOCOL_ERROR", "응답 본문이 JSON이 아님");
  }
}

export function connect(base: string, initialToken: string | null, fetchImpl: typeof fetch = fetch, options: { identityKinds?: Readonly<Record<string,"uuid"|"bigint">>; contractFingerprint?: string; idWire?: IdWireKind; maxResponseBytes?: number; validateRead?: (query: unknown, rows: unknown[]) => void; validateWrite?: (request: unknown, response: unknown) => void } = {}) {
  const expectedFingerprint = options.contractFingerprint;
  const resultWire = options.idWire;
  const validateRead = options.validateRead;
  const validateWrite = options.validateWrite;
  const maxResponseBytes = options.maxResponseBytes ?? DEFAULT_MAX_RESPONSE_BYTES;
  if (!Number.isSafeInteger(maxResponseBytes) || maxResponseBytes <= 0) {
    throw Object.assign(new Error("응답 크기 상한 형식 오류"), { code: "CLIENT.PROTOCOL_ERROR" });
  }
  if (resultWire !== undefined && !isIdWire(resultWire)) {
    throw Object.assign(new Error("Id wire 형식 오류"), { code: "CLIENT.PROTOCOL_ERROR" });
  }
  if (expectedFingerprint !== undefined && (typeof expectedFingerprint !== "string" || !/^[a-f0-9]{64}$/.test(expectedFingerprint))) {
    throw Object.assign(new Error("계약 식별값 형식 오류"), { code: "CLIENT.PROTOCOL_ERROR" });
  }
  let token = initialToken;
  let generation = 0;
  let principal: string | null | undefined;
  let checking: Promise<string | null> | undefined;
  let renewalQueue: Promise<unknown> = Promise.resolve();
  const cache = createCache();
  cache.setActor(`${generation}:${token}`);
  const pending = new Map<string, Pending>();
  const flights = new Map<string, { text: string; promise: Promise<any> }>();
  function freezeOutcome(res: any) {
    if (resultWire === undefined) return res;
    if (res.ok) for (const field of ["changed", "unchanged", "tags"]) Object.freeze(res[field]);
    if (res.ok && Array.isArray(res.applied)) {
      for (const row of res.applied) { Object.freeze(row.changed); Object.freeze(row.unchanged); Object.freeze(row); }
      Object.freeze(res.applied);
    }
    if (res.ok && Array.isArray(res.created)) { for (const row of res.created) Object.freeze(row); Object.freeze(res.created); }
    if (res.ok && Object.hasOwn(res, "output")) Object.freeze(res.output);
    return Object.freeze(res);
  }
  async function postWithToken(path: string, body: unknown, credential: string | null, extra: Record<string, string> = {}) {
    const headers: Record<string, string> = { "content-type": "application/json", ...extra };
    if (credential) headers["authorization"] = `Bearer ${credential}`;
    if (expectedFingerprint !== undefined && (["/read", "/apply", "/status", "/extension", "/operation"].includes(path) || MANAGED_PATHS.has(path))) {
      headers["x-aip-contract"] = expectedFingerprint;
    }
    // A service never redirects an API call; following one would resend the body (and for 307/308 the whole request) to another origin.
    const r = await fetchImpl(base + path, { method: "POST", headers, body: JSON.stringify(body), redirect: "error", credentials: "omit" });
    const parsed = await readJsonBounded(r, maxResponseBytes);
    // The body contract is untouched; the id travels in a header and is linked to the parsed body for the failures built from it.
    rememberRequestId(parsed, r);
    return parsed;
  }
  const post = (path: string, body: unknown, extra: Record<string, string> = {}) => postWithToken(path, body, token, extra);
  async function describe(credential: string | null): Promise<string | null> {
    const res = await postWithToken("/session", {}, credential);
    if (res?.ok === false && typeof res.code === "string") throw failure(res.code, res.code, res);
    const id = res?.principal?.actorId;
    const canonical = typeof id === "string" && (/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id) || id.length <= 20 && /^(0|[1-9][0-9]*|-[1-9][0-9]*)$/.test(id) && BigInt(id) >= -(1n << 63n) && BigInt(id) < (1n << 63n));
    const valid = credential === null ? id === null && res?.remainingMs === null : canonical && Number.isFinite(res?.remainingMs) && res.remainingMs > 0;
    if (res?.ok !== true || !valid) throw failure("CLIENT.PROTOCOL_ERROR", "세션 응답 계약 오류", res);
    return id;
  }
  async function ensurePrincipal() {
    if (principal !== undefined) return principal;
    if (!checking) {
      checking = describe(token).then((id) => (principal = id)).finally(() => { checking = undefined; });
    }
    return checking;
  }
  async function send(record: Pending, key: string, auth: AuthContext, extra: Record<string, string> = {}) {
    record.attempts++;
    try {
      const res = await postWithToken("/apply", { request: record.request, key }, auth.token, extra);
      if (auth.generation !== generation) return null;
      record.lastRetry = retryOf(res) ?? record.lastRetry;
      const attemptId = requestIdOf(res);
      if (attemptId !== undefined) record.lastRequestId = attemptId;
      const extensionWrite = record.request !== null && typeof record.request === "object" && Object.hasOwn(record.request, "extension");
      if (!validWriteResponse(res, resultWire, extensionWrite,record.request,options.identityKinds)) return null;
      if (res.ok && extensionWrite) {
        if (!validateWrite) return null;
        validateWrite(record.request, res);
      }
      return UNSETTLED.has(res.code) || (record.attempts > 1 && NOT_EXECUTED.has(res.code)) ? null : res;
    } catch {
      return null;
    }
  }
  function settle(key: string, record: Pending, res: any) {
    // A failed outcome names its request so that the rejection can be reported; a success carries nothing extra.
    const requestId = requestIdOf(res);
    if (res?.ok === false && requestId !== undefined && res.requestId === undefined) res = { ...res, requestId };
    if (res?.ok === false && Object.hasOwn(res, "retry") && retryOf(res) === undefined) {
      res = { ...res };
      delete res.retry;
    }
    res = freezeOutcome(res);
    record.outcome = res;
    if (pending.get(key) === record) {
      pending.delete(key);
      cache.resolveUnknown();
    }
    if (res.ok) cache.onWrite({ status: "ok", changed: res.tags });
    return res;
  }
  async function runApply(record: Pending, key: string, auth: AuthContext, opts: ApplyOptions) {
    let res = await send(record, key, auth, opts.dropResponse ? { "x-spike-drop-response": "1" } : {});
    if (record.outcome) return record.outcome;
    if (res) return settle(key, record, res);
    for (let i = 0; i < (opts.retries ?? 2); i++) {
      if (auth.generation !== generation) break;
      res = await send(record, key, auth);
      if (record.outcome) return record.outcome;
      if (res) return freezeOutcome({ ...settle(key, record, res), recovered: true });
    }
    throw new WriteUnsettled(key, structuredClone(record.request), record.lastRequestId, record.lastRetry);
  }
  async function apply(request: unknown, opts: ApplyOptions = {}) {
    // 전송 전 직렬화 오류는 미확정 쓰기가 아니다. JSON으로 고정해 호출자 변경과 재시도를 분리한다.
    const text = JSON.stringify(request);
    const snapshot = JSON.parse(text);
    // WriteUnsettled 뒤에 키 없이 같은 요청을 다시 부르면 새 키는 이미 커밋된 쓰기를 한 번 더 실행한다. 같은 본문의
    // 미확정 쓰기가 있으면 그 키를 쓴다. 같은 요청을 의도적으로 두 번 쓰려면 호출자가 서로 다른 key를 준다.
    let key = opts.key;
    if (key === undefined) {
      for (const [pendingKey, record] of pending) {
        if (JSON.stringify(record.request) === text) {
          key = pendingKey;
          break;
        }
      }
    }
    key ??= crypto.randomUUID();
    await ensurePrincipal();
    const flightKey = `${generation}:${key}`;
    const flight = flights.get(flightKey);
    let record = pending.get(key);
    if ((flight && flight.text !== text) || (record && JSON.stringify(record.request) !== text)) {
      return freezeOutcome({ ok: false, code: "WIRE.IDEMPOTENCY_MISMATCH" });
    }
    if (flight) return flight.promise;
    if (!record) {
      record = { request: snapshot, attempts: 0 };
      pending.set(key, record);
      cache.onWrite({ status: "unknown" });
    }
    const promise = runApply(record, key, { token, generation }, opts).finally(() => flights.delete(flightKey));
    flights.set(flightKey, { text, promise });
    return promise;
  }
  async function retryPending() {
    const out = [];
    for (const [key, record] of [...pending]) {
      try {
        out.push(freezeOutcome({ ...(await apply(record.request, { key, retries: 0 })), key }));
      } catch (e) {
        if (!(e instanceof WriteUnsettled)) throw e;
      }
    }
    return resultWire === undefined ? out : Object.freeze(out);
  }
  function replaceSession(nextToken: string) {
    const operation = renewalQueue.then(async () => {
      const current = await ensurePrincipal();
      const next = await describe(nextToken);
      if (current === null || next !== current) throw new ScopeConflict("다른 사용자 세션으로 미확정 쓰기를 이관할 수 없음");
      token = nextToken;
      generation++;
      cache.setActor(`${generation}:${token}`);
      return retryPending();
    });
    renewalQueue = operation.catch(() => {});
    return operation;
  }
  async function callPure(path: "/extension" | "/operation", field: "extension" | "operation", name: string, input: unknown): Promise<unknown> {
      const snapshot = structuredClone(input);
      await ensurePrincipal();
      const auth = { token, generation };
      const r = await postWithToken(path, { [field]: name, input: snapshot }, auth.token);
      if (auth.generation !== generation) throw new ScopeChanged("확장을 읽는 동안 세션이 바뀜");
      if (r?.ok === false && typeof r.code === "string") {
        if (r.code === "WIRE.CONTRACT_MISMATCH") cache.invalidate();
        throw failure(r.code, r.code, r);
      }
      if (r?.ok !== true) throw failure("CLIENT.PROTOCOL_ERROR", "확장 응답 형식 오류", r);
      if (expectedFingerprint === undefined || r.contractFingerprint !== expectedFingerprint) {
        cache.invalidate();
        throw failure("WIRE.CONTRACT_MISMATCH", "생성 확장 계약과 서버 계약이 다름", r);
      }
      return r.output as unknown;
  }
  async function callManaged(path: string, body: unknown): Promise<unknown> {
    if (!MANAGED_PATHS.has(path)) {
      throw Object.assign(new Error("지원하지 않는 관리 경로"), { code: "CLIENT.PROTOCOL_ERROR" });
    }
    await ensurePrincipal();
    const auth = { token, generation };
    const response = await postWithToken(path, structuredClone(body), auth.token);
    if (auth.generation !== generation) throw new ScopeChanged("관리 요청 중 세션이 바뀜");
    if (response?.ok === false && typeof response.code === "string") {
      if (response.code === "WIRE.CONTRACT_MISMATCH") cache.invalidate();
      throw failure(response.code, response.code, response);
    }
    if (response?.ok !== true) throw failure("CLIENT.PROTOCOL_ERROR", "관리 응답 형식 오류", response);
    if (expectedFingerprint === undefined || response.contractFingerprint !== expectedFingerprint) {
      cache.invalidate();
      throw failure("WIRE.CONTRACT_MISMATCH", "생성 관리 계약과 서버 계약이 다름", response);
    }
    return response;
  }
  return {
    cache,
    post,
    pending: () => [...pending.keys()],
    /**
     * 미확정 쓰기를 버린다. 서버가 그 요청을 실행하지 않았다고 확신할 때만 쓴다(예: 같은 키에 `BAD_REQUEST`를 받았다).
     * 커밋됐을 수 있는 쓰기를 버리면 결과를 영영 알 수 없다. 알 수 없는 키면 false다.
     */
    discardPending(key: string) {
      if (!pending.delete(key)) return false;
      flights.delete(`${generation}:${key}`);
      cache.resolveUnknown();
      return true;
    },
    async read(query: unknown) {
      const snapshot = structuredClone(query);
      await ensurePrincipal();
      const auth = { token, generation };
      return cache.read(snapshot, async () => {
        const r = await postWithToken("/read", { query: snapshot }, auth.token);
        // 구세대 응답은 새 세대의 계약 검사·캐시 무효화에도 참여하지 않는다.
        if (auth.generation !== generation) throw new ScopeChanged("읽는 동안 세션이 바뀜");
        if (r?.ok === false && typeof r.code === "string") {
          if (r.code === "WIRE.CONTRACT_MISMATCH") cache.invalidate();
          throw failure(r.code, r.code, r);
        }
        if (r?.ok !== true) throw failure("CLIENT.PROTOCOL_ERROR", "읽기 응답 계약 오류", r);
        if (expectedFingerprint !== undefined && r.contractFingerprint !== expectedFingerprint) {
          cache.invalidate();
          throw failure("WIRE.CONTRACT_MISMATCH", "생성 계약과 서버 계약이 다름", r);
        }
        if (!Array.isArray(r.rows) || !Array.isArray(r.deps) || !r.deps.every((d: unknown) => typeof d === "string") ||
          (r.maxAgeMs !== undefined && (typeof r.maxAgeMs !== "number" || !Number.isFinite(r.maxAgeMs) || r.maxAgeMs < 0))) {
          throw failure("CLIENT.PROTOCOL_ERROR", "읽기 응답 계약 오류", r);
        }
        validateRead?.(snapshot, r.rows);
        return { rows: r.rows, deps: r.deps, maxAgeMs: r.maxAgeMs };
      });
    },
    callExtension: (name: string, input: unknown) => callPure("/extension", "extension", name, input),
    callOperation: (name: string, input: unknown) => callPure("/operation", "operation", name, input),
    callManaged,
    apply,
    replaceSession,
    /** 미확정 쓰기를 같은 키로 다시 확정한다(네트워크 복구 뒤). */
    retryPending,
  };
}
