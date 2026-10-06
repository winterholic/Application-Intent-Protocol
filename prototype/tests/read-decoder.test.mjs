import {test} from 'node:test';
import assert from 'node:assert/strict';
import {connectTypedExtensions} from '../../spikes/spike-v6-transport/client/typed.ts';

const fp = 'a'.repeat(64);
const scalar = (type, nullable=false, values=undefined) => ({type, nullable, ...(values ? {values} : {})});
const reads = {
  Event: {root:true, maxRows:20, fields:{id:scalar('Id'), checked:scalar('Bool'), count:scalar('Int'), phase:scalar('Enum',false,['READY','DONE']), date:scalar('Time'), title:scalar('Text',true)}, traverse:{member:{target:'Member',select:['id','name']}}},
  Member:{root:false,maxRows:20,fields:{id:scalar('Id'),name:scalar('Text')},traverse:{}}
};
const query={read:'Event',select:['id','checked']};
function fixture(rows, wire='decimal-string-v13', handler) {
  let calls=0;
  const binding={fingerprint:fp,idWire:wire,readDescriptors:structuredClone(reads),extensions:{}};
  const client=connectTypedExtensions('http://decoder.invalid','token',binding,async (url,init)=>{
    if(url.endsWith('/session')) return new Response(JSON.stringify({ok:true,principal:{actorId:'1'},remainingMs:60000}));
    calls++;
    if(handler) await handler(init);
    return new Response(JSON.stringify({ok:true,rows,deps:['Event','Member'],maxAgeMs:60000,contractFingerprint:fp}));
  });
  return {client,binding,calls:()=>calls};
}
const protocol=e=>e?.code==='PROTOCOL_ERROR';

test('malformed selected fields and unsolicited fields never reach the read cache',async()=>{
  for(const row of [{id:11,checked:false},{id:'11',checked:'false'},{id:'11'},{id:'11',checked:false,privateNote:'secret'},null,[],{id:'011',checked:false},{id:'9223372036854775808',checked:false}]){
    const {client,calls}=fixture([row]);
    await assert.rejects(client.read(query),protocol);
    assert.equal(client.cache.size(),0);
    await assert.rejects(client.read(query),protocol);
    assert.equal(calls(),2);
  }
});
test('a valid selected row and cache hit are frozen and exact',async()=>{
  const {client}=fixture([{id:'11',checked:false}]);
  const first=await client.read(query);
  assert.equal(first.stored,true);
  assert.ok(Object.isFrozen(first.rows[0]));
  assert.equal((await client.read(query)).cached,true);
});
test('selected relations accept null or exact target records and reject private fields',async()=>{
  const selection={read:'Event',select:['id',{member:{select:['id','name']}}]};
  for(const member of [null,{id:'1',name:'member'}]){
    const {client}=fixture([{id:'11',member}]);
    assert.deepEqual((await client.read(selection)).rows,[{id:'11',member}]);
  }
  for(const member of [[],{id:'1'}, {id:'1',name:1},{id:'1',name:'member',email:'private'}]){
    const {client}=fixture([{id:'11',member}]);
    await assert.rejects(client.read(selection),protocol);
    assert.equal(client.cache.size(),0);
  }
});
test('existing empty and duplicate relation selections retain their server meaning',async()=>{
  for(const [select, member] of [[[],{}],[[],null],[['id','id'],{id:'1'}]]){
    const {client}=fixture([{member}]);
    assert.deepEqual((await client.read({read:'Event',select:[{member:{select}}]})).rows,[{member}]);
  }
});
test('prototype bindings must carry public read descriptors',()=>{
  assert.throws(()=>connectTypedExtensions('http://decoder.invalid','token',{fingerprint:fp,idWire:'decimal-string-v13',extensions:{}}),protocol);
});
test('read scalars honor nullable enums integer precision and server Time output strings',async()=>{
  for(const [field,value] of [['count',9007199254740992],['count',1.1],['phase','UNKNOWN'],['date',42],['title','a\0b'],['checked',null]]){
    const {client}=fixture([{[field]:value}]);
    await assert.rejects(client.read({read:'Event',select:[field]}),protocol);
    assert.equal(client.cache.size(),0);
  }
  for(const date of ['10000-01-01T13:59:59+00:00','0001-12-31T10:00:00+00:00 BC']){
    const {client}=fixture([{title:null,date}]);
    assert.deepEqual((await client.read({read:'Event',select:['title','date']})).rows,[{title:null,date}]);
  }
});
test('read Id validation uses the captured wire and read descriptors',async()=>{
  const {client,binding}=fixture([{id:'11',checked:false}]);
  binding.idWire='safe-number-v13';binding.readDescriptors.Event.fields.checked.type='Text';
  assert.deepEqual((await client.read(query)).rows,[{id:'11',checked:false}]);
  const bad=fixture([{id:9007199254740992,checked:false}],'safe-number-v13');
  await assert.rejects(bad.client.read(query),protocol);
});
test('the read query snapshot remains the sent and validated selection while caller mutates it',async()=>{
  const mutable={read:'Event',select:['id','checked']};
  let sent;
  const {client}=fixture([{id:'11',checked:false}],'decimal-string-v13',async init=>{sent=JSON.parse(init.body).query;mutable.select=['title'];});
  assert.deepEqual((await client.read(mutable)).rows,[{id:'11',checked:false}]);
  assert.deepEqual(sent,query);
  assert.equal(client.cache.size(),1);
});
