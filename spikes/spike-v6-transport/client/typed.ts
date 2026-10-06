import { connect, isIdWire, validResultId, type ApplyOptions } from "./transport.ts";
import type { ContractBinding, ContractShape, Root, SelectItem, CheckSel, FilterItem, SortItem, Row, DeepReadonly, ApplyBinding, ApplyContractShape, ApplyRequest, ApplyResult, PendingResult, IdWireKind, ExtensionContractShape, ExtensionDescriptors, ScalarDescriptor, ReadDescriptors, WriteExtensionDescriptors, WriteExtensionResult } from "../../spike-v5-sdk/sdk/generic.ts";

export function connectTypedApply<C extends ContractShape<C>, A extends ApplyContractShape<A>>(base: string, token: string | null, binding: ApplyBinding<C, A>, fetchImpl: typeof fetch = fetch) {
  const idWire = binding?.idWire;
  if (!isIdWire(idWire)) throw Object.assign(new Error("생성 쓰기 계약의 Id wire 형식 오류"), { code: "PROTOCOL_ERROR" });
  const client = typedClient(base, token, binding, fetchImpl, idWire);
  return applyFacade<C, A>(client);
}

function applyFacade<C extends ContractShape<C>, A extends ApplyContractShape<A>>(client: ReturnType<typeof typedClient<C>>) {
  return {
    ...client,
    apply<const K extends keyof A & string>(request: ApplyRequest<A, K> & { readonly apply: K }, options?: ApplyOptions): Promise<ApplyResult<A[K]["idOutput"]>> {
      return client.apply(request, options);
    },
    retryPending(): Promise<readonly PendingResult<A>[]> { return client.retryPending(); },
    replaceSession(token: string): Promise<readonly PendingResult<A>[]> { return client.replaceSession(token); },
  };
}

export type ReadEnvelope<T> = {
  readonly rows: readonly DeepReadonly<T>[];
  readonly cached: boolean;
  readonly stale: boolean;
  readonly stored?: boolean;
};

export function connectTyped<C extends ContractShape<C>>(base: string, token: string | null, binding: ContractBinding<C>, fetchImpl: typeof fetch = fetch) {
  return typedClient(base, token, binding, fetchImpl);
}

function typedClient<C extends ContractShape<C>>(base: string, token: string | null, binding: ContractBinding<C>, fetchImpl: typeof fetch, idWire?: IdWireKind, validateWrite?: (request: unknown, response: unknown) => void) {
  const fingerprint = binding?.fingerprint;
  if (typeof fingerprint !== "string" || !/^[a-f0-9]{64}$/.test(fingerprint)) {
    throw Object.assign(new Error("생성 계약 식별값 형식 오류"), { code: "PROTOCOL_ERROR" });
  }
  const descriptors = binding.readDescriptors === undefined ? undefined : structuredClone(binding.readDescriptors);
  const client = connect(base, token, fetchImpl, {
    contractFingerprint: fingerprint, idWire, validateWrite,
    validateRead: descriptors === undefined ? undefined : (query, rows) => validateReadRows(query, rows, descriptors, idWire ?? "legacy"),
  });
  return {
    ...client,
    cache: { size: client.cache.size },
    read<R extends Root<C>, const S extends readonly SelectItem<C, R>[]>(query: {
      readonly read: R;
      readonly select: S & CheckSel<S>;
      readonly filter?: readonly FilterItem<C, R>[];
      readonly sort?: readonly SortItem<C, R>[];
      readonly limit?: number;
    }): Promise<ReadEnvelope<Row<C, R, S>>> {
      return client.read(query) as Promise<ReadEnvelope<Row<C, R, S>>>;
    },
  };
}

function validTime(value: string): boolean {
  if (value.length < 20 || value.length > 35) return false;
  const m = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:[Zz]|([+-])(\d{2}):(\d{2}))$/.exec(value);
  if (!m) return false;
  const [year, month, day, hour, minute, second] = m.slice(1, 7).map(Number);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return year > 0 && month >= 1 && month <= 12 && day >= 1 && day <= days[month - 1] && hour <= 23 && minute <= 59 && second <= 60 &&
    (!m[7] || Number(m[8]) <= 23 && Number(m[9]) <= 59);
}

function validScalar(value: unknown, d: ScalarDescriptor, wire: IdWireKind): boolean {
  if (value === null) return d.nullable === true;
  switch (d.type) {
    case "Id": case "Ref":
      return wire === "legacy" ? typeof value === "string" && /^[0-9]+$/.test(value) : validResultId(value, wire);
    case "Bool": return typeof value === "boolean";
    // The JS decoder rejects an Int that cannot retain its exact integer value.
    case "Int": return typeof value === "number" && Number.isSafeInteger(value);
    case "Text": case "Url": return typeof value === "string" && !value.includes("\0");
    case "Time": return typeof value === "string" && validTime(value);
    case "Enum": return typeof value === "string" && Array.isArray(d.values) && d.values.includes(value);
    default: return false;
  }
}

function validateRecord(value: unknown, fields: Readonly<Record<string, ScalarDescriptor>>, wire: IdWireKind, code: string): asserts value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value) || !fields || typeof fields !== "object") {
    throw Object.assign(new Error("확장 값은 계약에 맞는 객체여야 함"), { code });
  }
  const record = value as Record<string, unknown>;
  if (Object.keys(record).length !== Object.keys(fields).length ||
      !Object.entries(fields).every(([key, d]) => Object.hasOwn(record, key) && d && validScalar(record[key], d, wire))) {
    throw Object.assign(new Error("확장 값이 생성 계약과 다름"), { code });
  }
}

type ReadDescription = { readonly root: boolean; readonly maxRows: number; readonly fields: Readonly<Record<string, ScalarDescriptor>>; readonly traverse: Readonly<Record<string, {readonly target: string; readonly select: readonly string[]}>> };

function validateReadRows(query: unknown, rows: unknown[], descriptors: Readonly<Record<string, ReadDescription>>, wire: IdWireKind) {
  function reject(): never { throw Object.assign(new Error("읽기 값이 생성 계약과 다름"), {code: "PROTOCOL_ERROR"}); }
  if (!query || typeof query !== "object" || Array.isArray(query)) reject();
  const request = query as {read: string; select: unknown[]; limit?: number};
  const resource = Object.hasOwn(descriptors, request.read) ? descriptors[request.read] : undefined;
  if (!resource?.root || !Array.isArray(request.select) || request.select.length === 0 ||
    (request.limit !== undefined && (!Number.isSafeInteger(request.limit) || request.limit < 1 || request.limit > resource.maxRows)) ||
    rows.length > Math.min(request.limit ?? resource.maxRows, resource.maxRows)) reject();
  function record(value: unknown, description: ReadDescription, selection: unknown[], allowed?: readonly string[]) {
    if (!value || typeof value !== "object" || Array.isArray(value)) reject();
    const row = value as Record<string, unknown>;
    const selected = new Set<string>();
    for (const item of selection) {
      const key = typeof item === "string" ? item : item && typeof item === "object" && !Array.isArray(item) && Object.keys(item).length === 1 ? Object.keys(item)[0] : reject();
      if (selected.has(key)) { if (allowed) continue; reject(); }
      if (!Object.hasOwn(row, key)) reject();
      selected.add(key);
      if (typeof item === "string") {
        if ((allowed && !allowed.includes(key)) || !Object.hasOwn(description.fields, key)) reject();
        const d = description.fields[key];
        const scalarValid = d.type === "Id" || d.type === "Ref" ? row[key] === null ? d.nullable : validResultId(row[key], wire)
          : d.type === "Time" ? row[key] === null ? d.nullable : typeof row[key] === "string" && !row[key].includes("\0")
          : validScalar(row[key], d, wire);
        if (!scalarValid) reject();
      } else {
        if (allowed || !Object.hasOwn(description.traverse, key)) reject();
        const relation = description.traverse[key];
        const target = Object.hasOwn(descriptors, relation.target) ? descriptors[relation.target] : undefined;
        const sub = (item as Record<string, unknown>)[key];
        if (!target || !sub || typeof sub !== "object" || Array.isArray(sub) || Object.keys(sub).length !== 1 || !Object.hasOwn(sub, "select")) reject();
        const fields = (sub as {select: unknown[]}).select;
        if (!Array.isArray(fields) || !fields.every(field => typeof field === "string" && relation.select.includes(field))) reject();
        if (row[key] !== null) record(row[key], target, fields, relation.select);
      }
    }
    if (Object.keys(row).length !== selected.size) reject();
  }
  for (const row of rows) record(row, resource, request.select);
}

type PendingWriteExtension<W extends ExtensionContractShape<W>> = keyof W extends never ? never : WriteExtensionResult<W[keyof W]["output"]>;

export function connectTypedExtensions<C extends ContractShape<C>, A extends ApplyContractShape<A>, E extends ExtensionContractShape<E> = {}, W extends ExtensionContractShape<W> = {}>(
  base: string, token: string | null,
  binding: ApplyBinding<C, A> & { readonly readDescriptors: ReadDescriptors<C>; readonly __extensions?: E; readonly extensions?: ExtensionDescriptors<E>; readonly __writes?: W; readonly writeExtensions?: WriteExtensionDescriptors<W> },
  fetchImpl: typeof fetch = fetch,
) {
  if (!binding?.readDescriptors || typeof binding.readDescriptors !== "object" || Array.isArray(binding.readDescriptors)) {
    throw Object.assign(new Error("생성 읽기 descriptor가 필요함. prototype binding을 다시 생성해야 함"), {code: "PROTOCOL_ERROR"});
  }
  const wire = binding.idWire;
  if (!isIdWire(wire)) throw Object.assign(new Error("생성 계약의 Id wire 형식 오류"), {code:"PROTOCOL_ERROR"});
  const fingerprint = binding.fingerprint;
  const descriptors = structuredClone(binding.extensions ?? {}) as ExtensionDescriptors<E>;
  const writes = structuredClone(binding.writeExtensions ?? {}) as WriteExtensionDescriptors<W>;
  const stable = {fingerprint, idWire:wire, readDescriptors:structuredClone(binding.readDescriptors)};
  const raw = typedClient<C>(base, token, stable, fetchImpl, wire, (request, response) => {
    const req = request as Record<string, unknown>;
    const res = response as Record<string, unknown>;
    const name = req.extension;
    if (typeof name !== "string" || !Object.hasOwn(writes, name) || res.contractFingerprint !== fingerprint) {
      throw Object.assign(new Error("생성 WRITE 응답 계약과 서버 계약이 다름"), {code:"PROTOCOL_ERROR"});
    }
    validateRecord(res.output, writes[name as keyof W].output, wire, "PROTOCOL_ERROR");
  });
  const { callExtension, ...client } = applyFacade<C,A>(raw);
  return {
    ...client,
    async extension<const K extends keyof E & string>(name: K, input: E[K]["input"]): Promise<DeepReadonly<E[K]["output"]>> {
      if (!Object.hasOwn(descriptors, name)) throw Object.assign(new Error("생성 계약에 없는 확장"), { code: "NOT_EXPOSED" });
      const descriptor = descriptors[name];
      const snapshot = structuredClone(input);
      validateRecord(snapshot, descriptor.input, wire, "BAD_VALUE");
      const output = await callExtension(name, snapshot);
      validateRecord(output, descriptor.output, wire, "PROTOCOL_ERROR");
      return Object.freeze(output) as DeepReadonly<E[K]["output"]>;
    },
    async writeExtension<const K extends keyof W & string>(name: K, input: W[K]["input"], options?: ApplyOptions): Promise<WriteExtensionResult<W[K]["output"]>> {
      if (!Object.hasOwn(writes, name)) throw Object.assign(new Error("생성 계약에 없는 WRITE 확장"), {code:"NOT_EXPOSED"});
      const snapshot = structuredClone(input);
      validateRecord(snapshot, writes[name].input, wire, "BAD_VALUE");
      return raw.apply({extension:name, input:snapshot}, options);
    },
    retryPending(): Promise<readonly ((PendingResult<A> | PendingWriteExtension<W>) & {readonly key:string})[]> { return raw.retryPending(); },
    replaceSession(token: string): Promise<readonly ((PendingResult<A> | PendingWriteExtension<W>) & {readonly key:string})[]> { return raw.replaceSession(token); },
  };
}
