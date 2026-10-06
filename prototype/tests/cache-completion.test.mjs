import {test} from 'node:test';
import assert from 'node:assert/strict';
import {connect} from '../sdk/index.ts';
import {createCache,ScopeChanged} from '../../spikes/spike-v5-sdk/sdk/cache.ts';

const fp='a'.repeat(64);
const turn=()=>new Promise(resolve=>setImmediate(resolve));
for(const wire of ['safe-number-v13','decimal-string-v13']){
  for(const extension of [false,true]){
    test(`${wire}: ${extension?'extension':'standard'} WRITE completion cannot re-cache an earlier read`,async()=>{
      const id=wire==='safe-number-v13'?1:'1';
      const scalar={type:'Id',nullable:false};
      const binding={fingerprint:fp,idWire:wire,readDescriptors:{Item:{root:true,maxRows:10,fields:{value:{type:'Text',nullable:false}},traverse:{}}},extensions:{},writeExtensions:{'Item.update':{input:{id:scalar},output:{id:scalar},access:['Item.update']}}};
      let resolveRead,resolveApply,reads=0;
      const client=connect('http://cache.invalid','token',binding,async url=>{
        if(url.endsWith('/session'))return {json:async()=>({ok:true,principal:{actorId:'1'},remainingMs:60000})};
        if(url.endsWith('/apply'))return {json:()=>new Promise(resolve=>{resolveApply=resolve})};
        if(url.endsWith('/read')){
          reads++;
          const fresh={ok:true,rows:[{value:'new'}],deps:['Item'],maxAgeMs:10000,contractFingerprint:fp};
          return reads>1?{json:async()=>fresh}:{json:()=>new Promise(resolve=>{resolveRead=resolve})};
        }
        throw new Error(url);
      });
      const opts={key:'write-completion',retries:0};
      const write=extension?client.writeExtension('Item.update',{id},opts):client.apply({apply:'Item.update',target:{ids:[id]}},opts);
      while(!resolveApply)await turn();
      const query={read:'Item',select:['value']};
      const read=client.read(query);
      while(!resolveRead)await turn();
      resolveRead({ok:true,rows:[{value:'old'}],deps:['Item'],maxAgeMs:10000,contractFingerprint:fp});
      resolveApply(extension?{ok:true,output:{id},tags:['Item'],contractFingerprint:fp}:{ok:true,changed:[id],unchanged:[],tags:['Item']});
      const [result,first]=await Promise.all([write,read]);
      assert.equal(result.ok,true);
      assert.deepEqual(client.pending(),[]);
      assert.deepEqual(first.rows,[{value:'new'}],'a completed write invalidates the in-flight old read');
      const second=await client.read(query);
      assert.equal(second.cached,true);
      assert.deepEqual(second.rows,[{value:'new'}]);
      assert.equal(reads,2,'read the current value once, then reuse that value');
    });
  }
}

test('a scope change between fetching and returning discards the old read',async()=>{
  const cache=createCache();cache.setActor('old');
  let resolve;
  const source=new Promise(done=>{resolve=done});
  const read=cache.read({read:'Item'},()=>source);
  resolve({rows:[{value:'old'}],deps:['Item'],maxAgeMs:10000});
  queueMicrotask(()=>cache.setActor('new'));
  await assert.rejects(read,ScopeChanged);
  assert.equal(cache.size(),0);
});

test('a related resource change at the return boundary triggers a fresh read',async()=>{
  const cache=createCache();cache.setActor('actor');
  let resolve,calls=0;
  const source=new Promise(done=>{resolve=done});
  const read=cache.read({read:'Item'},()=>++calls===1?source:Promise.resolve({rows:[{value:'new'}],deps:['Item'],maxAgeMs:10000}));
  resolve({rows:[{value:'old'}],deps:['Item'],maxAgeMs:10000});
  queueMicrotask(()=>cache.onWrite({status:'ok',changed:['Item']}));
  assert.deepEqual((await read).rows,[{value:'new'}]);
  assert.equal(calls,2);
});
