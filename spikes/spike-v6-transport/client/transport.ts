// V6 클라이언트(실험). 전송 + V5 캐시.
// 쓰기 응답을 확정하지 못하면(통신 실패, COMMIT_UNKNOWN·INTERNAL) 같은 키·같은 본문으로 다시 보낸다.
// 서버는 키 단위로 직렬화하므로 커밋됐으면 저장 결과, 진행 중이면 끝난 뒤 결과, 롤백됐으면 새 실행을 돌려준다.
// 확정 응답을 받기 전까지 그 키는 미확정으로 남고 캐시 저장도 보류된다(r7 R7-02·R7-03).
import { createCache, ScopeChanged } from "../../spike-v5-sdk/sdk/cache.ts";
import type { IdWireKind } from "../../spike-v5-sdk/sdk/generic.ts";

export type ApplyOptions = { key?: string; dropResponse?: boolean; retries?: number };
export function isIdWire(value: unknown): value is IdWireKind {
  return value === "legacy" || value === "safe-number-v13" || value === "decimal-string-v13";
}

export function validResultId(value: unknown, wire: IdWireKind): boolean {
  if (wire === "decimal-string-v13") {
    return typeof value === "string" && value.length <= 19 && /^(0|[1-9][0-9]*)$/.test(value) && BigInt(value) <= (1n << 63n) - 1n;
  }
  if (wire === "safe-number-v13") return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
  return typeof value === "number" && Number.isInteger(value) && value >= -(2 ** 63) && value <= 2 ** 63;
}

function validWriteResponse(res: any, wire?: IdWireKind, extensionWrite = false): boolean {
  const metadata = wire === undefined || (res?.replayed === undefined || typeof res.replayed === "boolean") && (res?.recovered === undefined || typeof res.recovered === "boolean");
  if (!metadata) return false;
  if (res?.ok === false) return typeof res.code === "string" && (wire === undefined || res.msg === undefined || typeof res.msg === "string");
  if (res?.ok !== true || !Array.isArray(res.tags) || !res.tags.every((tag: unknown) => typeof tag === "string")) return false;
  if (extensionWrite) return res.output !== null && typeof res.output === "object" && !Array.isArray(res.output);
  return wire === undefined || [res.changed, res.unchanged].every(ids => Array.isArray(ids) && ids.every(id => validResultId(id, wire)));
}

const UNSETTLED = new Set(["COMMIT_UNKNOWN", "INTERNAL"]);
// 실행 전 거부나 이번 시도의 rollback은 같은 키의 이전 커밋 여부를 알려 주지 않는다.
const NOT_EXECUTED = new Set(["TOKEN_EXPIRED", "UNAUTHENTICATED", "BAD_REQUEST", "PAYLOAD_TOO_LARGE", "NOT_FOUND", "CONTRACT_MISMATCH", "ORIGIN_NOT_ALLOWED", "REQUEST_TIMEOUT", "DB_UNAVAILABLE", "WORKER_BUSY", "DEADLINE_EXCEEDED", "CONFLICT"]);

export class WriteUnsettled extends Error {
  key: string;
  request: unknown;
  constructor(key: string, request: unknown) {
    super("쓰기 결과를 아직 확정하지 못함");
    this.key = key;
    this.request = request;
  }
}

export class ScopeConflict extends Error {
  name = "ScopeConflict";
}

type Pending = { request: unknown; attempts: number; outcome?: any };
type AuthContext = { token: string | null; generation: number };

// 미확정 쓰기는 서버 검증 principal에, 캐시·진행 중 요청은 인증 세대에 귀속한다.
export function connect(base: string, initialToken: string | null, fetchImpl: typeof fetch = fetch, options: { contractFingerprint?: string; idWire?: IdWireKind; validateRead?: (query: unknown, rows: unknown[]) => void; validateWrite?: (request: unknown, response: unknown) => void } = {}) {
  const expectedFingerprint = options.contractFingerprint;
  const resultWire = options.idWire;
  const validateRead = options.validateRead;
  const validateWrite = options.validateWrite;
  if (resultWire !== undefined && !isIdWire(resultWire)) {
    throw Object.assign(new Error("Id wire 형식 오류"), { code: "PROTOCOL_ERROR" });
  }
  if (expectedFingerprint !== undefined && (typeof expectedFingerprint !== "string" || !/^[a-f0-9]{64}$/.test(expectedFingerprint))) {
    throw Object.assign(new Error("계약 식별값 형식 오류"), { code: "PROTOCOL_ERROR" });
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
    if (res.ok && Object.hasOwn(res, "output")) Object.freeze(res.output);
    return Object.freeze(res);
  }
  async function postWithToken(path: string, body: unknown, credential: string | null, extra: Record<string, string> = {}) {
    const headers: Record<string, string> = { "content-type": "application/json", ...extra };
    if (credential) headers["authorization"] = `Bearer ${credential}`;
    if (expectedFingerprint !== undefined && ["/read", "/apply", "/status", "/extension", "/operation"].includes(path)) {
      headers["x-aip-contract"] = expectedFingerprint;
    }
    const r = await fetchImpl(base + path, { method: "POST", headers, body: JSON.stringify(body) });
    return (await r.json()) as any;
  }
  const post = (path: string, body: unknown, extra: Record<string, string> = {}) => postWithToken(path, body, token, extra);
  async function describe(credential: string | null): Promise<string | null> {
    const res = await postWithToken("/session", {}, credential);
    if (res?.ok === false && typeof res.code === "string") throw Object.assign(new Error(res.code), { code: res.code });
    const id = res?.principal?.actorId;
    const canonical = typeof id === "string" && id.length <= 20 && /^(0|[1-9][0-9]*|-[1-9][0-9]*)$/.test(id) && BigInt(id) >= -(1n << 63n) && BigInt(id) < (1n << 63n);
    const valid = credential === null ? id === null && res?.remainingMs === null : canonical && Number.isFinite(res?.remainingMs) && res.remainingMs > 0;
    if (res?.ok !== true || !valid) throw Object.assign(new Error("세션 응답 계약 오류"), { code: "PROTOCOL_ERROR" });
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
      const extensionWrite = record.request !== null && typeof record.request === "object" && Object.hasOwn(record.request, "extension");
      if (!validWriteResponse(res, resultWire, extensionWrite)) return null;
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
    throw new WriteUnsettled(key, structuredClone(record.request));
  }
  async function apply(request: unknown, opts: ApplyOptions = {}) {
    const key = opts.key ?? crypto.randomUUID();
    // 전송 전 직렬화 오류는 미확정 쓰기가 아니다. JSON으로 고정해 호출자 변경과 재시도를 분리한다.
    const text = JSON.stringify(request);
    const snapshot = JSON.parse(text);
    await ensurePrincipal();
    const flightKey = `${generation}:${key}`;
    const flight = flights.get(flightKey);
    let record = pending.get(key);
    if ((flight && flight.text !== text) || (record && JSON.stringify(record.request) !== text)) {
      return freezeOutcome({ ok: false, code: "IDEMPOTENCY_MISMATCH" });
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
        if (r.code === "CONTRACT_MISMATCH") cache.invalidate();
        throw Object.assign(new Error(r.code), { code: r.code });
      }
      if (r?.ok !== true) throw Object.assign(new Error("확장 응답 형식 오류"), { code: "PROTOCOL_ERROR" });
      if (expectedFingerprint === undefined || r.contractFingerprint !== expectedFingerprint) {
        cache.invalidate();
        throw Object.assign(new Error("생성 확장 계약과 서버 계약이 다름"), { code: "CONTRACT_MISMATCH" });
      }
      return r.output as unknown;
  }
  return {
    cache,
    post,
    pending: () => [...pending.keys()],
    async read(query: unknown) {
      const snapshot = structuredClone(query);
      await ensurePrincipal();
      const auth = { token, generation };
      return cache.read(snapshot, async () => {
        const r = await postWithToken("/read", { query: snapshot }, auth.token);
        // 구세대 응답은 새 세대의 계약 검사·캐시 무효화에도 참여하지 않는다.
        if (auth.generation !== generation) throw new ScopeChanged("읽는 동안 세션이 바뀜");
        if (r?.ok === false && typeof r.code === "string") {
          if (r.code === "CONTRACT_MISMATCH") cache.invalidate();
          throw Object.assign(new Error(r.code), { code: r.code });
        }
        if (r?.ok !== true) throw Object.assign(new Error("읽기 응답 계약 오류"), { code: "PROTOCOL_ERROR" });
        if (expectedFingerprint !== undefined && r.contractFingerprint !== expectedFingerprint) {
          cache.invalidate();
          throw Object.assign(new Error("생성 계약과 서버 계약이 다름"), { code: "CONTRACT_MISMATCH" });
        }
        if (!Array.isArray(r.rows) || !Array.isArray(r.deps) || !r.deps.every((d: unknown) => typeof d === "string") ||
          (r.maxAgeMs !== undefined && (typeof r.maxAgeMs !== "number" || !Number.isFinite(r.maxAgeMs) || r.maxAgeMs < 0))) {
          throw Object.assign(new Error("읽기 응답 계약 오류"), { code: "PROTOCOL_ERROR" });
        }
        validateRead?.(snapshot, r.rows);
        return { rows: r.rows, deps: r.deps, maxAgeMs: r.maxAgeMs };
      });
    },
    callExtension: (name: string, input: unknown) => callPure("/extension", "extension", name, input),
    callOperation: (name: string, input: unknown) => callPure("/operation", "operation", name, input),
    apply,
    replaceSession,
    /** 미확정 쓰기를 같은 키로 다시 확정한다(네트워크 복구 뒤). */
    retryPending,
  };
}
