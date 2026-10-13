import type { ManagedSend } from "./infrastructure.ts";
import { failure } from "./request-id.ts";

export type RagCitation = Readonly<{ sourceId: string; documentId: string; chunkIndex: number; source: string }>;
export type RagHit = Readonly<{ text: string; score: number; citation: RagCitation }>;
export type RagAnswer = Readonly<{ answer: string; citations: readonly RagCitation[] }>;
export type RagSearchOptions = Readonly<{ topK?: number; minScore?: number }>;

function fail(code: string): never { throw Object.assign(new Error(code), {code}); }
function record(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === "object" && !Array.isArray(value); }
function text(value: unknown, max: number): value is string { return typeof value === "string" && value.length > 0 && new TextEncoder().encode(value).length <= max && !value.includes("\0"); }
function citation(value: unknown): RagCitation {
  if (!record(value) || !text(value.sourceId,256) || !text(value.documentId,256) || !text(value.source,2048) ||
    !Number.isSafeInteger(value.chunkIndex) || (value.chunkIndex as number) < 0) fail("CLIENT.PROTOCOL_ERROR");
  return Object.freeze({sourceId:value.sourceId,documentId:value.documentId,source:value.source,chunkIndex:value.chunkIndex as number});
}
function options(value: RagSearchOptions): RagSearchOptions {
  if (!record(value) || Object.keys(value).some(key=>!['topK','minScore'].includes(key)) ||
    (value.topK !== undefined && (!Number.isSafeInteger(value.topK) || value.topK < 1 || value.topK > 100)) ||
    (value.minScore !== undefined && (!Number.isFinite(value.minScore) || value.minScore < -1 || value.minScore > 1))) fail("WIRE.BAD_VALUE");
  return {...value};
}

export function createRagClient(send: ManagedSend, fingerprint: string) {
  if (typeof send !== "function" || !/^[a-f0-9]{64}$/.test(fingerprint)) fail("WIRE.BAD_VALUE");
  async function call(path: string, body: Record<string, unknown>) {
    if (!text(body.corpus,128) || !/^[A-Za-z0-9_.-]+$/.test(body.corpus)) fail("WIRE.BAD_VALUE");
    const result = await send(path,body);
    if (!record(result)) fail("CLIENT.PROTOCOL_ERROR");
    if (result.ok === false && typeof result.code === "string") throw failure(result.code, result.code, result);
    if (result.ok !== true) throw failure("CLIENT.PROTOCOL_ERROR", "CLIENT.PROTOCOL_ERROR", result);
    if (result.contractFingerprint !== fingerprint) fail("WIRE.CONTRACT_MISMATCH");
    return result;
  }
  return Object.freeze({
    async search(corpus: string, query: string, searchOptions: RagSearchOptions = {}): Promise<readonly RagHit[]> {
      if (!text(query,8192)) fail("WIRE.BAD_VALUE");
      const opts=options(searchOptions);
      const result=await call("/rag/search",{corpus,query,...opts});
      if (!Array.isArray(result.hits) || result.hits.length > (opts.topK ?? 5)) fail("CLIENT.PROTOCOL_ERROR");
      return Object.freeze(result.hits.map((hit: unknown)=>{
        if (!record(hit) || !text(hit.text,8192) || typeof hit.score !== "number" || !Number.isFinite(hit.score) || Math.abs(hit.score)>1.000001) fail("CLIENT.PROTOCOL_ERROR");
        return Object.freeze({text:hit.text,score:hit.score,citation:citation(hit.citation)});
      }));
    },
    async answer(corpus: string, question: string, searchOptions: RagSearchOptions = {}): Promise<RagAnswer> {
      if (!text(question,8192)) fail("WIRE.BAD_VALUE");
      const opts=options(searchOptions);
      const result=await call("/rag/answer",{corpus,question,...opts});
      if (!text(result.answer,1<<20) || !Array.isArray(result.citations) || result.citations.length>(opts.topK ?? 5)) fail("CLIENT.PROTOCOL_ERROR");
      return Object.freeze({answer:result.answer,citations:Object.freeze(result.citations.map(citation))});
    },
    async index(corpus: string, documentId: string, source: string, content: string): Promise<number> {
      if (!text(documentId,256) || !text(source,2048) || !text(content,900*1024)) fail("WIRE.BAD_VALUE");
      const result=await call("/rag/index",{corpus,documentId,source,text:content});
      if (!Number.isSafeInteger(result.chunks) || (result.chunks as number)<0) fail("CLIENT.PROTOCOL_ERROR");
      return result.chunks as number;
    },
    async delete(corpus: string, documentId: string): Promise<number> {
      if (!text(documentId,256)) fail("WIRE.BAD_VALUE");
      const result=await call("/rag/delete",{corpus,documentId});
      if (!Number.isSafeInteger(result.deleted) || (result.deleted as number)<0) fail("CLIENT.PROTOCOL_ERROR");
      return result.deleted as number;
    },
  });
}
