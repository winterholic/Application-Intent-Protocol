import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTypedExtensions } from "./typed.ts";
import type { OperationBinding } from "../../spike-v5-sdk/sdk/generic.ts";

type Operations = { length: { input: {text:string}; output: {length:number} } };
const fingerprint = "a".repeat(64);
const binding: OperationBinding<{}, {}, {}, {}, Operations> = {
  fingerprint,idWire:"safe-number-v13",readDescriptors:{},extensions:{},
  operations:{length:{input:{text:{type:"Text",nullable:false}},output:{length:{type:"Int",nullable:false}},lifetime:"sync",effect:"none",deadlineMs:2000,dependencies:{worker:[],authorization:["database","principal","deployment"]}}},
};
const response = (body:unknown) => new Response(JSON.stringify(body));

test("operation validates typed input and output with the generated contract", async () => {
  const calls:string[]=[];
  let badOutput=false;
  const fetcher:typeof fetch=async(url,init)=>{
    const path=new URL(String(url)).pathname;
    if(path==="/session") return response({ok:true,principal:{actorId:"1"},remainingMs:60000});
    assert.equal(path,"/operation");
    assert.equal(new Headers(init?.headers).get("x-aip-contract"),fingerprint);
    assert.deepEqual(JSON.parse(String(init?.body)),{operation:"length",input:{text:"가😀A"}});
    calls.push(path);
    return response({ok:true,output:{length:badOutput?"three":3},contractFingerprint:fingerprint});
  };
  const client=connectTypedExtensions("http://operation.invalid","token",binding,fetcher);
  const output=await client.operation("length",{text:"가😀A"});
  assert.deepEqual(output,{length:3});
  assert.ok(Object.isFrozen(output));
  await assert.rejects(client.operation("length",{text:42} as never),(error:any)=>error.code==="BAD_VALUE");
  await assert.rejects(client.operation("missing" as never,{} as never),(error:any)=>error.code==="NOT_EXPOSED");
  assert.equal(calls.length,1,"invalid calls must not reach the server");
  badOutput=true;
  await assert.rejects(client.operation("length",{text:"가😀A"}),(error:any)=>error.code==="PROTOCOL_ERROR");
});

test("operation rejects mismatched contracts and responses after a session replacement", async () => {
  let finish!:(response:Response)=>void;
  const fetcher:typeof fetch=async(url)=>{
    if(new URL(String(url)).pathname==="/session") return response({ok:true,principal:{actorId:"1"},remainingMs:60000});
    return new Promise(resolve=>{finish=resolve;});
  };
  const client=connectTypedExtensions("http://operation.invalid","old",binding,fetcher);
  const pending=client.operation("length",{text:"가😀A"});
  while(!finish) await new Promise(resolve=>setImmediate(resolve));
  await client.replaceSession("new");
  finish(response({ok:true,output:{length:3},contractFingerprint:fingerprint}));
  await assert.rejects(pending,(error:any)=>error.name==="ScopeChanged");
  const mismatch=connectTypedExtensions("http://operation.invalid","token",binding,async(url)=>response(new URL(String(url)).pathname==="/session"?{ok:true,principal:{actorId:"1"},remainingMs:60000}:{ok:true,output:{length:3},contractFingerprint:"b".repeat(64)}));
  await assert.rejects(mismatch.operation("length",{text:"가😀A"}),(error:any)=>error.code==="CONTRACT_MISMATCH");
});

test("operation descriptor ranges reject invalid Unicode length and numeric output", async () => {
  const ranged: typeof binding = {...binding, operations: {length: {...binding.operations.length,
    input: {text: {type:"Text",nullable:false,range:[1,3]}},
    output: {length: {type:"Int",nullable:false,range:[1,3]}},
  }}};
  const fetcher:typeof fetch=async(url)=>response(new URL(String(url)).pathname==="/session"?{ok:true,principal:{actorId:"1"},remainingMs:60000}:{ok:true,output:{length:4},contractFingerprint:fingerprint});
  const client=connectTypedExtensions("http://operation.invalid","token",ranged,fetcher);
  await assert.rejects(client.operation("length",{text:"가😀AB"}),(error:any)=>error.code==="BAD_VALUE");
  await assert.rejects(client.operation("length",{text:"가😀A"}),(error:any)=>error.code==="PROTOCOL_ERROR");
});
