import { test } from "node:test";
import assert from "node:assert/strict";
import { connectTyped } from "./typed.ts";
import { contract } from "./generated-v13-string.ts";
import { WriteUnsettled } from "./transport.ts";

const response=(body:unknown)=>new Response(JSON.stringify(body));
const session={ok:true,principal:{actorId:"1"},remainingMs:60000};
const read={ok:true,rows:[{id:"42"}],deps:["Item"],maxAgeMs:1000,contractFingerprint:contract.fingerprint};

test("typed 계약 기대값은 read/apply/status에 전달하고 처음 거부는 pending을 남기지 않음",async()=>{
  const seen:string[]=[];
  const fetcher:typeof fetch=async(input,init)=>{
    const path=new URL(String(input)).pathname;
    const header=new Headers(init?.headers).get("x-aip-contract");
    if(path==="/session") { assert.equal(header,null); return response(session); }
    assert.equal(header,contract.fingerprint);
    seen.push(path);
    return response(path==="/read"?read:{ok:false,code:"CONTRACT_MISMATCH"});
  };
  const client=connectTyped("http://binding.invalid","token",contract,fetcher);
  await client.read({read:"Item",select:["id"]});
  const rejected=await client.apply({apply:"Item.read",target:{where:[]}},{key:"first",retries:0});
  assert.equal(rejected.code,"CONTRACT_MISMATCH");
  assert.deepEqual(client.pending(),[]);
  await client.post("/status",{key:"first",request:{apply:"Item.read",target:{where:[]}}});
  assert.deepEqual(seen,["/read","/apply","/status"]);
});

test("이전 응답 유실 뒤 계약 거부는 앞선 commit을 확정하지 않아 pending 보존",async()=>{
  let attempts=0;
  const fetcher:typeof fetch=async(input)=>{
    const path=new URL(String(input)).pathname;
    if(path==="/session") return response(session);
    if(path==="/apply") {
      if(++attempts===1) throw new Error("응답 유실");
      return response({ok:false,code:"CONTRACT_MISMATCH"});
    }
    return response(read);
  };
  const client=connectTyped("http://binding.invalid","token",contract,fetcher);
  const request={apply:"Item.read",target:{ids:["42"]}};
  await assert.rejects(client.apply(request,{key:"unknown",retries:0}),WriteUnsettled);
  await assert.rejects(client.apply(request,{key:"unknown",retries:0}),WriteUnsettled);
  assert.deepEqual(client.pending(),["unknown"]);
  const fresh=await client.read({read:"Item",select:["id"]});
  assert.equal(fresh.stored,false);
});

test("서버의 계약 사전 거부도 현재 세대의 기존 캐시를 비움",async()=>{
  let reads=0;
  const fetcher:typeof fetch=async(input)=>{
    const path=new URL(String(input)).pathname;
    if(path==="/session") return response(session);
    return response(++reads===2?{ok:false,code:"CONTRACT_MISMATCH"}:read);
  };
  const client=connectTyped("http://binding.invalid","token",contract,fetcher);
  await client.read({read:"Item",select:["id"]});
  assert.equal((await client.read({read:"Item",select:["id"]})).cached,true);
  await assert.rejects(client.read({read:"Item",select:["label"]}),(e:any)=>e.code==="CONTRACT_MISMATCH");
  assert.equal((await client.read({read:"Item",select:["id"]})).cached,false);
  assert.equal(reads,3);
});
