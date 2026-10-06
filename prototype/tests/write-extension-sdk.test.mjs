import {test} from 'node:test';
import assert from 'node:assert/strict';
import {connect} from '../sdk/index.ts';
import {WriteUnsettled} from '../../spikes/spike-v6-transport/client/transport.ts';

const fp='c'.repeat(64);
const response=value=>new Response(JSON.stringify(value));
function binding(wire) {
  return {fingerprint:fp,idWire:wire,extensions:{},
    readDescriptors:{Event:{root:true,maxRows:10,fields:{id:{type:'Id',nullable:false},checked:{type:'Bool',nullable:false}},traverse:{}}},
    writeExtensions:{'Event.run':{input:{id:{type:'Id',nullable:false}},output:{id:{type:'Id',nullable:false},count:{type:'Int',nullable:false}},access:['Event.mark']}}};
}
const session={ok:true,principal:{actorId:'1'},remainingMs:60_000};
for (const wire of ['safe-number-v13','decimal-string-v13']) {
  const id=wire==='safe-number-v13'?11:'11';
  test(`${wire}: WRITE shares read cache, freezes output and snapshots binding/input`,async()=>{
    const metadata=binding(wire);let reads=0;const sent=[];let changed=false;
    const client=connect('http://write.invalid','token',metadata,async(url,init)=>{
      if(url.endsWith('/session'))return response(session);
      if(url.endsWith('/read')){reads++;return response({ok:true,rows:[{id,checked:changed}],deps:['Event'],maxAgeMs:60_000,contractFingerprint:fp});}
      sent.push(JSON.parse(init.body));changed=true;
      return response({ok:true,output:{id,count:1},tags:['Event'],contractFingerprint:fp});
    });
    metadata.idWire=wire==='safe-number-v13'?'decimal-string-v13':'safe-number-v13';metadata.writeExtensions['Event.run'].output.count.type='Text';
    const query={read:'Event',select:['id','checked']};await client.read(query);assert.equal((await client.read(query)).cached,true);
    let accesses=0;const input={get id(){accesses++;return accesses===1?id:(typeof id==='string'?'99':99);}};
    const result=await client.writeExtension('Event.run',input,{key:'write-key',retries:0});
    assert.equal(result.ok,true);assert.deepEqual(result.output,{id,count:1});assert.equal(accesses,1);
    assert.deepEqual(sent,[{request:{extension:'Event.run',input:{id}},key:'write-key'}]);
    assert.ok(Object.isFrozen(result)&&Object.isFrozen(result.output)&&Object.isFrozen(result.tags));assert.deepEqual(client.pending(),[]);
    const after=await client.read(query);assert.equal(after.cached,false);assert.equal(after.rows[0].checked,true);assert.equal(reads,2);
  });
  test(`${wire}: malformed WRITE success stays pending until validated replay`,async()=>{
    let attempts=0;const sent=[];
    const client=connect('http://write.invalid','token',binding(wire),async(url,init)=>{
      if(url.endsWith('/session'))return response(session);
      sent.push(JSON.parse(init.body));attempts++;
      return response({ok:true,output:{id,count:attempts===1?9007199254740992:1},tags:['Event'],contractFingerprint:fp,replayed:attempts>1});
    });
    await assert.rejects(client.writeExtension('Event.run',{id},{key:'lost',retries:0}),WriteUnsettled);assert.deepEqual(client.pending(),['lost']);
    const recovered=await client.retryPending();assert.equal(recovered[0].ok,true);assert.equal(recovered[0].replayed,true);assert.deepEqual(recovered[0].output,{id,count:1});assert.deepEqual(client.pending(),[]);
    assert.deepEqual(sent,[{request:{extension:'Event.run',input:{id}},key:'lost'},{request:{extension:'Event.run',input:{id}},key:'lost'}]);
  });
  test(`${wire}: WRITE input/name rejection happens before network`,async()=>{
    let calls=0;const client=connect('http://write.invalid','token',binding(wire),async()=>{calls++;throw new Error('should not send');});
    const wrong=wire==='safe-number-v13'?'11':11;
    await assert.rejects(client.writeExtension('Event.run',{id:wrong}),error=>error.code==='BAD_VALUE');
    await assert.rejects(client.writeExtension('Event.missing',{id}),error=>error.code==='NOT_EXPOSED');assert.equal(calls,0);assert.deepEqual(client.pending(),[]);
  });
  test(`${wire}: wrong WRITE response fingerprint cannot settle a write`,async()=>{
    const client=connect('http://write.invalid','token',binding(wire),async url=>response(url.endsWith('/session')?session:{ok:true,output:{id,count:1},tags:['Event'],contractFingerprint:'d'.repeat(64)}));
    await assert.rejects(client.writeExtension('Event.run',{id},{key:'wrong-fp',retries:0}),WriteUnsettled);assert.deepEqual(client.pending(),['wrong-fp']);
  });
}
