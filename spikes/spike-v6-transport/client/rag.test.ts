import assert from "node:assert/strict";
import { test } from "node:test";
import { createRagClient } from "./rag.ts";

const fingerprint = "a".repeat(64);
test("RAG sends only named corpus and owner-free inputs and verifies response contracts", async () => {
  const calls: unknown[] = [];
  const rag = createRagClient(async (path, body) => {
    calls.push([path, body]);
    return {ok:true,contractFingerprint:fingerprint,hits:[{text:"manual",score:1,citation:{sourceId:"S1",documentId:"doc",chunkIndex:0,source:"manual"}}]};
  },fingerprint);
  const hits = await rag.search("manuals","backend",{topK:1});
  assert.equal(hits[0].citation.documentId,"doc");
  assert.deepEqual(calls, [["/rag/search",{corpus:"manuals",query:"backend",topK:1}]]);
  assert(Object.isFrozen(hits[0].citation));
  await assert.rejects(rag.search("manuals","backend",{topK:0}), {code:"WIRE.BAD_VALUE"});
  const mismatch=createRagClient(async()=>({ok:true,hits:[],contractFingerprint:"b".repeat(64)}),fingerprint);
  await assert.rejects(mismatch.search("manuals","query"),{code:"WIRE.CONTRACT_MISMATCH"});
});
test("RAG respects server errors and rejects malformed answer citations", async()=>{
  const denied=createRagClient(async()=>({ok:false,code:"WIRE.NOT_AVAILABLE"}),fingerprint);
  await assert.rejects(denied.index("manuals","doc","manual","text"),{code:"WIRE.NOT_AVAILABLE"});
  const malformed=createRagClient(async()=>({ok:true,answer:"text",citations:[{}],contractFingerprint:fingerprint}),fingerprint);
  await assert.rejects(malformed.answer("manuals","query"),{code:"CLIENT.PROTOCOL_ERROR"});
});
