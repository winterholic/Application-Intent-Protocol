import { isIdWire, validResultId } from "./transport.ts";
import type { DeepReadonly, ExtensionContractShape, IdWireKind, OperationDescriptors, ScalarDescriptor } from "../../spike-v5-sdk/sdk/generic.ts";

export type JobsTransport = (route: string, body: unknown) => Promise<unknown>;
export type JobsBinding<O extends ExtensionContractShape<O>> = {
  readonly fingerprint: string;
  readonly idWire: IdWireKind;
  readonly operations: OperationDescriptors<O>;
};
export type JobStatus = "QUEUED" | "RUNNING" | "CANCEL_REQUESTED" | "SUCCEEDED" | "FAILED" | "CANCELLED";
export type EnqueueOptions = { readonly key: string; readonly delayMs?: number };
export type JobReceipt = DeepReadonly<{ id: string; status: JobStatus; error?: string }>;
export type JobResult<O extends ExtensionContractShape<O>, K extends keyof O> = DeepReadonly<O[K]["output"]>;
export type EventPage = DeepReadonly<{
  events: readonly { cursor: string; payload: unknown }[];
  cursor: string;
}>;
export type JobEvent = DeepReadonly<{ cursor: string; payload: unknown }>;
export type EventWatchOptions = { readonly after?: string; readonly signal?: AbortSignal; readonly intervalMs?: number };
export type FileMetadata = DeepReadonly<{ id: string; bucket: string; name: string; mediaType: string; size: number }>;
export type RetrievedFile = DeepReadonly<{ id: string; bucket: string; name: string; mediaType: string; size: number; bytes: Uint8Array }>;
type JobRun = { id: string; state?: JobStatus; status?: JobStatus; operation: string; attempt: number; error?: string | null };

const MAX_FILE_BYTES = 768 * 1024 - 1024;
const JOB_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const JOB_STATUSES = new Set<JobStatus>(["QUEUED", "RUNNING", "CANCEL_REQUESTED", "SUCCEEDED", "FAILED", "CANCELLED"]);
const RETRYABLE_EVENT_ERRORS = new Set(["DB_UNAVAILABLE", "REQUEST_TIMEOUT", "COMMIT_UNKNOWN", "WORKER_BUSY", "INTERNAL", "OFFLINE", "NETWORK_ERROR", "SERVICE_UNAVAILABLE"]);

function error(code: string, message = code): Error & { code: string } {
  return Object.assign(new Error(message), { code });
}

function wait(ms: number, signal?: AbortSignal): Promise<boolean> {
  if (signal?.aborted) return Promise.resolve(false);
  return new Promise(resolve => {
    const finish = () => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      resolve(!signal?.aborted);
    };
    const onAbort = () => finish();
    const timer = setTimeout(finish, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

function retryableEventError(value: unknown): boolean {
  if (!value || typeof value !== "object") return false;
  const candidate = value as { code?: unknown; name?: unknown };
  if (candidate.name === "AbortError") return false;
  if (typeof candidate.code === "string") return RETRYABLE_EVENT_ERRORS.has(candidate.code);
  return value instanceof TypeError;
}

function deepFreeze<T>(value: T): DeepReadonly<T> {
  if (value && typeof value === "object" && !ArrayBuffer.isView(value) && !Object.isFrozen(value)) {
    for (const child of Object.values(value as Record<string, unknown>)) deepFreeze(child);
    Object.freeze(value);
  }
  return value as DeepReadonly<T>;
}

function validDate(value: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) return false;
  const [year, month, day] = match.slice(1).map(Number);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return year > 0 && month >= 1 && month <= 12 && day >= 1 && day <= days[month - 1];
}

function validTime(value: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:[Zz]|([+-])(\d{2}):(\d{2}))$/.exec(value);
  if (!match || value.length > 35) return false;
  const [year, month, day, hour, minute, second] = match.slice(1, 7).map(Number);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return year > 0 && month >= 1 && month <= 12 && day >= 1 && day <= days[month - 1] && hour <= 23 && minute <= 59 && second <= 60 &&
    (!match[7] || Number(match[8]) <= 23 && Number(match[9]) <= 59);
}

function validScalar(value: unknown, descriptor: ScalarDescriptor, wire: IdWireKind): boolean {
  if (!descriptor || typeof descriptor.type !== "string" || typeof descriptor.nullable !== "boolean") return false;
  if (value === null) return descriptor.nullable;
  if (descriptor.range !== undefined) {
    const [min, max] = descriptor.range;
    const measure = typeof value === "string" ? [...value].length : typeof value === "number" ? value : NaN;
    if (!Number.isSafeInteger(min) || !Number.isSafeInteger(max) || min > max || !Number.isSafeInteger(measure) || measure < min || measure > max) return false;
  }
  if (descriptor.type.startsWith("Decimal<")) {
    const match = /^Decimal<(0|[1-9][0-9]*),(0|[1-9][0-9]*)>$/.exec(descriptor.type);
    if (!match || typeof value !== "string") return false;
    const precision = Number(match[1]), scale = Number(match[2]);
    const decimal = /^(-?)(0|[1-9][0-9]*)(?:\.([0-9]+))?$/.exec(value);
    return !!decimal && precision >= 1 && precision <= 38 && scale <= precision && (decimal[3]?.length ?? 0) <= scale &&
      (scale !== 0 || decimal[3] === undefined) && (decimal[2] === "0" ? 0 : decimal[2].length) <= precision - scale;
  }
  switch (descriptor.type) {
    case "Id": case "Ref":
      return wire === "legacy" ? typeof value === "string" && /^[0-9]+$/.test(value) : validResultId(value, wire);
    case "Bool": return typeof value === "boolean";
    case "Int": return typeof value === "number" && Number.isSafeInteger(value);
    case "Text": case "Url": return typeof value === "string" && !value.includes("\0");
    case "Email": {
      if (typeof value !== "string" || value.includes("\0") || /\p{White_Space}/u.test(value)) return false;
      const at = value.indexOf("@");
      const domain = value.slice(at + 1);
      return at > 0 && at === value.lastIndexOf("@") && domain.length > 0 && domain.includes(".") && !domain.startsWith(".") && !domain.endsWith(".");
    }
    case "Date": return typeof value === "string" && validDate(value);
    case "Time": return typeof value === "string" && validTime(value);
    case "Enum": return typeof value === "string" && Array.isArray(descriptor.values) && descriptor.values.includes(value);
    default: return false;
  }
}

function validateRecord(value: unknown, fields: Readonly<Record<string, ScalarDescriptor>>, wire: IdWireKind, code: string): asserts value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value) || Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null ||
      Object.keys(value).length !== Object.keys(fields).length ||
      !Object.entries(fields).every(([key, descriptor]) => Object.hasOwn(value, key) && validScalar((value as Record<string, unknown>)[key], descriptor, wire))) {
    throw error(code, code === "BAD_VALUE" ? "입력이 operation 계약과 다릅니다" : "응답이 operation 계약과 다릅니다");
  }
}

function contractResponse<T extends Record<string, unknown>>(value: unknown, fingerprint: string): T {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw error("PROTOCOL_ERROR", "jobs 응답 형식이 잘못되었습니다");
  const response = value as Record<string, unknown>;
  if (response.ok === false && typeof response.code === "string") throw error(response.code, typeof response.msg === "string" ? response.msg : response.code);
  if (response.ok !== true) throw error("PROTOCOL_ERROR", "jobs 응답 형식이 잘못되었습니다");
  if (response.contractFingerprint !== fingerprint) throw error("CONTRACT_MISMATCH", "서버와 생성 계약이 다릅니다");
  return response as T;
}

function validJobId(value: unknown): value is string {
  return typeof value === "string" && JOB_ID.test(value);
}

function validateStatus(value: unknown): asserts value is JobRun {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw error("PROTOCOL_ERROR", "job 상태 응답이 잘못되었습니다");
  const job = value as Record<string, unknown>;
  const status = job.status ?? job.state;
  if (!validJobId(job.id) || typeof status !== "string" || !JOB_STATUSES.has(status as JobStatus) ||
      typeof job.operation !== "string" || job.error != null && (typeof job.error !== "string" || job.error.length > 256) || !Number.isSafeInteger(job.attempt) || (job.attempt as number) < 0) {
    throw error("PROTOCOL_ERROR", "job 상태 응답이 잘못되었습니다");
  }
}

function jobStatus(value: JobRun): JobStatus {
  return (value.status ?? value.state) as JobStatus;
}

function validText(value: unknown, max = 512): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= max && !value.includes("\0");
}

function encodeBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  return btoa(binary);
}

function decodeBase64(value: unknown): Uint8Array {
  if (typeof value !== "string" || value.length > Math.ceil(MAX_FILE_BYTES / 3) * 4 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) {
    throw error("PROTOCOL_ERROR", "파일 응답의 base64가 잘못되었습니다");
  }
  const binary = atob(value);
  if (binary.length > MAX_FILE_BYTES) throw error("PROTOCOL_ERROR", "파일 응답이 허용 크기를 초과했습니다");
  const bytes = Uint8Array.from(binary, char => char.charCodeAt(0));
  if (encodeBase64(bytes) !== value) throw error("PROTOCOL_ERROR", "파일 응답의 base64가 정규 형식이 아닙니다");
  return bytes;
}

/** Create a typed client for durable jobs and the accompanying event and file routes. */
export function createJobsClient<O extends ExtensionContractShape<O>>(post: JobsTransport, binding: JobsBinding<O>) {
  if (!binding || typeof binding.fingerprint !== "string" || !/^[a-f0-9]{64}$/.test(binding.fingerprint) || !isIdWire(binding.idWire) ||
      !binding.operations || typeof binding.operations !== "object" || Array.isArray(binding.operations)) {
    throw error("PROTOCOL_ERROR", "생성 jobs 계약 형식이 잘못되었습니다");
  }
  const fingerprint = binding.fingerprint;
  const wire = binding.idWire;
  const operations = structuredClone(binding.operations);
  async function send<T extends Record<string, unknown>>(route: string, body: unknown): Promise<T> {
    return contractResponse<T>(await post(route, body), fingerprint);
  }

  async function status(id: string): Promise<JobReceipt> {
    if (!validJobId(id)) throw error("BAD_VALUE", "job id 형식이 잘못되었습니다");
    const response = await send<Record<string, unknown>>("/jobs/status", { id });
    const run = response.run;
    validateStatus(run);
    if (run.id !== id || !Object.hasOwn(operations, run.operation)) throw error("PROTOCOL_ERROR", "응답 job id 또는 operation이 요청과 다릅니다");
    return deepFreeze({ id, status: jobStatus(run), ...(run.error ? { error: run.error } : {}) });
  }

  async function cancel(id: string): Promise<JobReceipt> {
    if (!validJobId(id)) throw error("BAD_VALUE", "job id 형식이 잘못되었습니다");
    const response = await send<Record<string, unknown>>("/jobs/cancel", { id });
    const run = response.run;
    validateStatus(run);
    if (run.id !== id || !Object.hasOwn(operations, run.operation)) throw error("PROTOCOL_ERROR", "응답 job id 또는 operation이 요청과 다릅니다");
    return deepFreeze({ id, status: jobStatus(run), ...(run.error ? { error: run.error } : {}) });
  }

  async function readEvents(stream: string, options: { readonly after?: string; readonly limit?: number } = {}): Promise<EventPage> {
    const after = options.after ?? "0";
    const limit = options.limit ?? 100;
    if (typeof stream !== "string" || !/^[A-Za-z][A-Za-z0-9_.-]{0,127}$/.test(stream) || typeof after !== "string" || after.length > 1024 || !Number.isSafeInteger(limit) || limit < 1 || limit > 100) {
      throw error("BAD_VALUE", "event cursor 또는 limit 형식이 잘못되었습니다");
    }
    const response = await send<Record<string, unknown>>("/events/read", { stream, after, limit });
    if (!Array.isArray(response.events) || typeof response.cursor !== "string" || response.cursor.length > 1024 ||
        !response.events.every(event => !!event && typeof event === "object" && !Array.isArray(event) && typeof (event as any).cursor === "string" && Object.hasOwn(event, "payload"))) {
      throw error("PROTOCOL_ERROR", "event 응답 형식이 잘못되었습니다");
    }
    return deepFreeze({ events: structuredClone(response.events), cursor: response.cursor }) as EventPage;
  }

  async function* watchEvents(stream: string, options: EventWatchOptions = {}): AsyncGenerator<JobEvent, void, unknown> {
    let after = options.after ?? "0";
    const intervalMs = options.intervalMs ?? 1000;
    const signal = options.signal;
    if (typeof stream !== "string" || !/^[A-Za-z][A-Za-z0-9_.-]{0,127}$/.test(stream) || typeof after !== "string" || after.length > 1024 ||
        !Number.isSafeInteger(intervalMs) || intervalMs < 50 || intervalMs > 30_000 ||
        (signal !== undefined && (!signal || typeof signal.aborted !== "boolean" || typeof signal.addEventListener !== "function"))) {
      throw error("BAD_VALUE", "event watch 인수가 잘못되었습니다");
    }
    let consecutiveFailures = 0;
    while (!signal?.aborted) {
      let page: EventPage;
      try {
        page = await readEvents(stream, { after, limit: 100 });
        consecutiveFailures = 0;
      } catch (cause) {
        if (signal?.aborted) return;
        if (!retryableEventError(cause) || consecutiveFailures >= 5) throw cause;
        consecutiveFailures++;
        const backoffMs = Math.min(intervalMs * 2 ** (consecutiveFailures - 1), 5000);
        if (!await wait(backoffMs, signal)) return;
        continue;
      }
      if (signal?.aborted) return;
      if (page.events.length === 0) {
        after = page.cursor;
        if (!await wait(intervalMs, signal)) return;
        continue;
      }
      for (const event of page.events) {
        if (signal?.aborted) return;
        after = event.cursor;
        yield deepFreeze(event) as JobEvent;
        if (signal?.aborted) return;
      }
    }
  }

  return {
    async enqueue<const K extends keyof O & string>(operation: K, input: O[K]["input"], options: EnqueueOptions): Promise<JobReceipt> {
      if (!Object.hasOwn(operations, operation)) throw error("NOT_EXPOSED", "생성 계약에 없는 operation입니다");
      const descriptor = operations[operation];
      if (descriptor.lifetime !== "durable") throw error("NOT_DURABLE", "durable로 선언되지 않은 operation입니다");
      const snapshot = structuredClone(input);
      validateRecord(snapshot, descriptor.input, wire, "BAD_VALUE");
      const key = options.key;
      const delayMs = options.delayMs ?? 0;
      if (!validText(key, 200) || !Number.isSafeInteger(delayMs) || delayMs < 0) throw error("BAD_VALUE", "job key 또는 delayMs 형식이 잘못되었습니다");
      const response = await send<Record<string, unknown>>("/jobs/enqueue", { operation, input: snapshot, key, delayMs });
      const run = response.run;
      validateStatus(run);
      if (run.operation !== operation) throw error("PROTOCOL_ERROR", "응답 operation이 요청과 다릅니다");
      return deepFreeze({ id: run.id, status: jobStatus(run), ...(run.error ? { error: run.error } : {}) });
    },
    status,
    cancel,
    async result<const K extends keyof O & string>(id: string, operation: K): Promise<JobResult<O, K>> {
      if (!validJobId(id)) throw error("BAD_VALUE", "job id 형식이 잘못되었습니다");
      if (!Object.hasOwn(operations, operation)) throw error("NOT_EXPOSED", "생성 계약에 없는 operation입니다");
      const response = await send<Record<string, unknown>>("/jobs/result", { id });
      if (response.id !== id || response.operation !== operation || !Object.hasOwn(response, "output")) throw error("PROTOCOL_ERROR", "job 결과 응답이 잘못되었습니다");
      const output = structuredClone(response.output);
      validateRecord(output, operations[operation].output, wire, "PROTOCOL_ERROR");
      return deepFreeze(output) as JobResult<O, K>;
    },
    events: { read: readEvents, watch: watchEvents },
    files: {
      async put(bucket: string, name: string, bytes: Uint8Array, mediaType: string): Promise<FileMetadata> {
        if (!validText(bucket) || !validText(name) || !validText(mediaType, 255) || !(bytes instanceof Uint8Array) || bytes.byteLength > MAX_FILE_BYTES) {
          throw error("BAD_VALUE", "파일 정보가 유효하지 않거나 업로드 크기가 허용 범위를 초과했습니다");
        }
        const response = await send<Record<string, unknown>>("/files/put", { bucket, name, mediaType, base64: encodeBase64(bytes) });
        const file = response.file as Record<string, unknown> | undefined;
        if (!file || !validText(file.id) || file.bucket !== bucket || file.name !== name || file.mediaType !== mediaType || file.size !== bytes.byteLength) throw error("PROTOCOL_ERROR", "파일 응답 메타데이터가 잘못되었습니다");
        return deepFreeze({ id: file.id, bucket, name, mediaType, size: bytes.byteLength });
      },
      async get(id: string): Promise<RetrievedFile> {
        if (!validText(id)) throw error("BAD_VALUE", "파일 id 형식이 잘못되었습니다");
        const response = await send<Record<string, unknown>>("/files/get", { id });
        const file = response.file as Record<string, unknown> | undefined;
        if (!file || file.id !== id || !validText(file.bucket) || !validText(file.name) || !validText(file.mediaType)) throw error("PROTOCOL_ERROR", "파일 메타데이터 응답이 잘못되었습니다");
        const bytes = decodeBase64(response.base64);
        if (file.size !== bytes.byteLength) throw error("PROTOCOL_ERROR", "파일 크기 metadata와 본문 길이가 다릅니다");
        return deepFreeze({ id, bucket: file.bucket, name: file.name, mediaType: file.mediaType, size: bytes.byteLength, bytes });
      },
      async delete(id: string): Promise<boolean> {
        if (!validText(id)) throw error("BAD_VALUE", "파일 id 형식이 잘못되었습니다");
        const response = await send<Record<string, unknown>>("/files/delete", { id });
        if (response.deleted !== true) throw error("PROTOCOL_ERROR", "파일 삭제 응답이 잘못되었습니다");
        return true;
      },
    },
  };
}
