import {test} from 'node:test';
import assert from 'node:assert/strict';
import {connectTypedApply} from './typed.ts';
const uuid='00000000-0000-4000-8000-000000000001';
test('a UUID cannot settle a numeric-resource write, while a declared UUID round-trips',async()=>{
 for (const kind of ['bigint','uuid'] as const) {
  const binding:any={fingerprint:'a'.repeat(64),idWire:'safe-number-v13',identityKinds:{Note:kind}};
  const calls:string[]=[];
  const request:any={apply:'Note.Archive',target:{ids:[kind==='uuid'?uuid:1]}};
  const fetcher:any=async(url:string)=>{calls.push(url);return new Response(JSON.stringify(url.endsWith('/session')?{ok:true,principal:{actorId:uuid},remainingMs:60000}:url.endsWith('/describe')?{ok:true,contractFingerprint:binding.fingerprint}:{ok:true,changed:[uuid],unchanged:[],tags:['Note']}),{headers:{'content-type':'application/json'}});};
  const client=connectTypedApply('', 'token',binding,fetcher);
  if(kind==='bigint') {await assert.rejects(client.apply(request,{key:'key',retries:0}));assert.equal(client.pending().length,1);}
  else {const out:any=await client.apply(request,{key:'key',retries:0});assert.deepEqual(out.changed,[uuid]);assert.equal(client.pending().length,0);}
 }
});
