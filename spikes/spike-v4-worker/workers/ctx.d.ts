// Types of the `ctx` an AIP extension (Node) receives: `export async function run(input, ctx)` in `<module>.mjs`.
// They describe workers/worker.mjs as it is: argument order, optional arguments and what each call resolves to.
// Use it from plain JavaScript with JSDoc, no build step:
//
//   /** @param {{ owner: string }} input
//    *  @param {import("./ctx").Ctx} ctx */
//   export async function run(input, ctx) { ... }
//
// This file is the common surface. It does not know your `access` list: a call the server will refuse for a missing grant
// (for example `create.Todo` not in `access`) still type-checks and ends in `ctx.<op>.access_not_declared` at run time.
// Narrowing by grant is the job of the generated SDK types, not of this file.

/**
 * An entity id. The server's id wire decides the form: a string on `decimal` (the service default) and `uuid`, a JS number on the
 * legacy wire. Compare ids as received; do not do arithmetic on them.
 */
export type EntityId = string | number;

/** What `apply` takes for a target id: `worker.mjs` turns every one into a string. */
export type TargetId = string | number | bigint;

/** One filter of a public read: the field must be listed in the resource's `expose read filter`. */
export interface ReadFilter {
  readonly field: string;
  readonly op: string;
  readonly value: unknown;
}

export interface ReadSort {
  readonly field: string;
  readonly dir: "asc" | "desc";
}

/** An item of `select`: a field name, or a relation with its own `select` (traverse). */
export type SelectItem = string | { readonly [relation: string]: { readonly select: readonly SelectItem[] } };

/** A public read (`access read.<Resource>`), planned with the caller's row and field policies. */
export interface ReadQuery {
  readonly read: string;
  readonly select: readonly SelectItem[];
  readonly filter?: readonly ReadFilter[];
  readonly sort?: readonly ReadSort[];
  readonly limit?: number;
  /** Only for a resource whose `expose read` budget sets `offset`. */
  readonly offset?: number;
  /** Only for a resource whose `expose read` budget sets `cursor`. */
  readonly after?: string;
}

export interface ReadResult {
  readonly rows: readonly Record<string, unknown>[];
}

/** Result of `ctx.data.apply`: the ids the transition changed and the ones that already held the target state. */
export interface ApplyResult {
  readonly changed: readonly EntityId[];
  readonly unchanged: readonly EntityId[];
}

export interface Data {
  /** A public aggregate of the caller's rows, `ctx.data.aggregate("Order", "revenue", {...})`. */
  aggregate(resource: string, name: string, input: Record<string, unknown>): Promise<unknown>;
  /**
   * A declared public transition on rows the caller can see (access `<Resource>.<transition>`), inside the server's transaction.
   * `key` names the call for you (checked: 1..100 characters, no control characters); the outer request's idempotency key already
   * covers the whole extension call. `values` and `expect` are for a patch transition (the new values and the values the rows must
   * still hold; a mismatch ends in `WIRE.CONFLICT`).
   * `input` is the transition's `input { ... }` block (P0-B): the amounts it adds or subtracts, every declared key required. The server
   * checks them (positive, inside the declared range) and computes on the locked row.
   */
  apply(
    resource: string,
    transition: string,
    ids: readonly TargetId[],
    key?: string,
    values?: Readonly<Record<string, unknown>>,
    expect?: Readonly<Record<string, unknown>>,
    input?: Readonly<Record<string, unknown>>,
  ): Promise<ApplyResult>;
  /** A public read of the caller's rows (access `read.<Resource>`); inside a write extension it sees the transaction's own writes. */
  read(query: ReadQuery): Promise<ReadResult>;
  /**
   * One row of the resource's `expose create` as the caller (access `create.<Resource>`): the create's field list, `allow`
   * and readable-reference rules are the client's. Resolves to the new row's id. `key` is only checked in a PostgreSQL write
   * extension (the same key twice in one call makes two rows); a durable job journals it.
   */
  create(resource: string, values: Readonly<Record<string, unknown>>, key?: string): Promise<EntityId | undefined>;
}

/** The last argument of a Mongo or Redis write: `true` keeps a way back for a failed relational commit; `{ key }` names a durable job's write. */
export type EffectOptions = true | { readonly compensate?: boolean; readonly key?: string };

export interface Mongo {
  find(collection: string, filter?: Record<string, unknown>, projection?: Record<string, unknown>, limit?: number): Promise<unknown[]>;
  findOne(collection: string, filter?: Record<string, unknown>, projection?: Record<string, unknown>): Promise<unknown>;
  countDocuments(collection: string, filter?: Record<string, unknown>): Promise<number>;
  insertOne(collection: string, document: Record<string, unknown>, compensate?: EffectOptions): Promise<unknown>;
  updateOne(
    collection: string,
    filter: Record<string, unknown>,
    update: Record<string, unknown>,
    upsert?: boolean,
    compensate?: EffectOptions,
  ): Promise<unknown>;
  findOneAndUpdate(
    collection: string,
    filter: Record<string, unknown>,
    update: Record<string, unknown>,
    upsert?: boolean,
    compensate?: EffectOptions,
  ): Promise<unknown>;
  deleteOne(collection: string, filter: Record<string, unknown>, compensate?: EffectOptions): Promise<number>;
}

export interface Redis {
  get(key: string): Promise<unknown>;
  hgetall(key: string): Promise<unknown>;
  exists(key: string): Promise<boolean>;
  mget(keys: readonly string[]): Promise<unknown>;
  lrange(key: string, start?: number, stop?: number): Promise<unknown[]>;
  replaceHashEx(key: string, fields: Record<string, unknown>, ttlSeconds: number, opts?: EffectOptions): Promise<number>;
  setEx(key: string, value: unknown, ttlSeconds: number, compensate?: EffectOptions): Promise<unknown>;
  setNxEx(key: string, value: unknown, ttlSeconds: number, compensate?: EffectOptions): Promise<boolean>;
  del(key: string, compensate?: EffectOptions): Promise<number>;
  getdel(key: string, compensate?: EffectOptions): Promise<unknown>;
  incrExpire(key: string, ttlSeconds: number, opts?: EffectOptions): Promise<unknown>;
  expire(key: string, ttlSeconds: number, opts?: EffectOptions): Promise<boolean>;
  pushTrimExpire(key: string, value: unknown, maxLen: number, ttlSeconds: number, opts?: EffectOptions): Promise<unknown>;
}

/** What `throw ctx.reject(...)` and `throw ctx.retryLater(...)` build: an error the worker reports with its code. */
export interface ExtensionRejection extends Error {
  readonly code: "WIRE.EXTENSION_REJECTED";
  readonly appCode: string;
}

export interface RetryLater extends Error {
  readonly code: "WIRE.RETRY_LATER";
  readonly afterMs: number | undefined;
}

export interface Ctx {
  readonly data: Data;
  readonly mongo: Mongo;
  readonly redis: Redis;
  /** Durable job only: an outbound call declared with `access http.<name>`. */
  readonly http: { call(name: string, input: unknown, key?: string): Promise<unknown> };
  /** Durable job only: a signed event to a subscriber URL (`access http.<name>`, server `outbound_webhooks`). */
  readonly webhooks: { deliver(name: string, url: string, payload: unknown, key: string): Promise<unknown> };
  readonly files: {
    put(bucket: string, input: Record<string, unknown>, key?: string): Promise<unknown>;
    get(bucket: string, id: string): Promise<unknown>;
  };
  readonly events: { publish(stream: string, key: string, payload: unknown): Promise<unknown> };
  readonly messages: { publish(queue: string, payload: unknown, key?: string): Promise<unknown> };
  readonly jobs: { run(name: string, input: unknown, key?: string): Promise<unknown> };
  /** Durable job only: the run id and the attempt number (from 1). `null` outside a job. */
  readonly job: { readonly id: EntityId; readonly attempt: number } | null;
  /**
   * `throw ctx.reject("INSUFFICIENT_STOCK", "재고가 부족합니다")`: a business refusal. The server undoes everything written and the
   * caller gets `WIRE.EXTENSION_REJECTED` with this code and text. The code is upper-case letters, digits and `_` (starts with a
   * letter, at most 48 bytes); the text is at most 200 characters without control characters. The server checks both.
   */
  reject(appCode: string, message?: string): ExtensionRejection;
  /** Durable job only: `throw ctx.retryLater("why", { afterMs })` runs the same job again after the backoff. */
  retryLater(reason?: string, options?: { readonly afterMs?: number }): RetryLater;
}

/** The shape of an exported extension function: `export const run: ExtensionFunction<In, Out> = async (input, ctx) => ...`. */
export type ExtensionFunction<Input = unknown, Output = unknown> = (input: Input, ctx: Ctx) => Output | Promise<Output>;
