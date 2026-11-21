import fs from 'node:fs';
import {post,show} from './raw.mjs';
const TOKEN=fs.readFileSync('token.txt','utf8').trim();
const FP='5e7fa7d8d69eda48cf02f7d9406f5664bbc3058e550278d8f734806105b36bc0';
async function raw(path, headers, body, method='POST'){
  const r=await fetch('http://127.0.0.1:18471'+path,{method,headers,body:body===undefined?undefined:JSON.stringify(body)});
  let t=await r.text(); let j; try{j=JSON.parse(t)}catch{j=t.slice(0,120)}
  return {status:r.status, rid:r.headers.get('x-request-id'), body:j};
}
const H={'content-type':'application/json',authorization:'Bearer '+TOKEN};
const q={query:{read:'Product',select:['id']}};
for (const [p,b] of [['/read',q],['/apply',{key:'h1',request:{apply:'Product.take',target:{ids:['1']},input:{qty:1}}}],['/status',{key:'h1'}]]){
  for (const [lab,fp] of [['none',undefined],['nonhex','zzz'],['wronghex','0'.repeat(64)],['UPPER-correct',FP.toUpperCase()]]){
    const h={...H}; if(fp!==undefined) h['x-aip-contract']=fp;
    const r=await raw(p,h,b); console.log(p,lab,r.status,r.body.code??('ok:'+r.body.ok),'rid=',r.rid===r.body.requestId);
  }
}
// success body has no requestId/retry
const ok=await raw('/read',{...H,'x-aip-contract':FP},q); console.log('success keys',Object.keys(ok.body), 'x-request-id' , ok.rid);
// SDK.md read claims
const Hc={...H,'x-aip-contract':FP};
const t=async(l,query)=>{const r=await raw('/read',Hc,{query}); console.log(l,r.status,r.body.code??'OK',(r.body.msg||'').slice(0,60))};
await t('dot select',{read:'OrderLine',select:['order.id']});
await t('filter object',{read:'Product',select:['id'],filter:{sku:'A'}});
await t('sort direction',{read:'Product',select:['id'],sort:[{field:'id',direction:'desc'}]});
await t('sort dir desc',{read:'Product',select:['id'],sort:[{field:'id',dir:'desc'}]});
await t('filter unopened',{read:'Product',select:['id'],filter:[{field:'stock',op:'eq',value:1}]});
await t('filter in 51',{read:'Product',select:['id'],filter:[{field:'sku',op:'in',value:Array.from({length:51},(_,i)=>'s'+i)}]});
await t('filter in 50',{read:'Product',select:['id'],filter:[{field:'sku',op:'in',value:Array.from({length:50},(_,i)=>'s'+i)}]});
await t('offset not allowed',{read:'Product',select:['id'],offset:1});
await t('after not allowed',{read:'Product',select:['id'],after:{id:'1'}});
await t('limit>budget',{read:'Product',select:['id'],limit:51});
await t('unknown key',{read:'Product',select:['id'],bogus:1});
await t('unknown resource',{read:'Nope',select:['id']});
await t('select ref string',{read:'OrderLine',select:['product']});
await t('no expose read',{read:'Note',select:['id']});
await t('unknown field',{read:'Product',select:['nope']});
// bundle limits
const ops=(n)=>Array.from({length:n},(_,i)=>({create:'Note',values:{owner:'42',text:'x'}}));
for (const n of [20,21]) { const r=await raw('/apply',Hc,{key:'lim'+n+Date.now(),request:{atomic:ops(n)}}); console.log('bundle ops',n,r.body.code??'ok'); }
const steps=(n)=>Array.from({length:n},()=>({transition:'confirm'}));
for (const n of [10,11]) { const r=await raw('/apply',Hc,{key:'cs'+n+Date.now(),request:{compose:'Order',targets:{ids:['3']},steps:steps(n)}}); console.log('compose steps',n,r.body.code??'ok', (r.body.msg||'').slice(0,50)); }
// refs rules
const k=()=>'k'+Math.random();
let r=await raw('/apply',Hc,{key:k(),request:{atomic:[{create:'OrderLine',values:{product:'1',qty:1,amount:'1.00'},refs:{order:{created:5}}}]}}); console.log('refs forward/out of range',r.body.code,(r.body.msg||'').slice(0,70));
r=await raw('/apply',Hc,{key:k(),request:{atomic:[{create:'Order',values:{owner:'42',total:'1.00',status:'open'}},{create:'OrderLine',values:{product:'1',qty:1,amount:'1.00',order:'1'},refs:{order:{created:0}}}]}}); console.log('refs+values same field',r.body.code,(r.body.msg||'').slice(0,70));
r=await raw('/apply',Hc,{key:k(),request:{atomic:[{create:'Note',values:{owner:'42'}},{create:'OrderLine',values:{product:'1',qty:1,amount:'1.00'},refs:{order:{created:0}}}]}}); console.log('refs wrong type (Note->Order)',r.body.code,(r.body.msg||'').slice(0,70));
r=await raw('/apply',Hc,{key:k(),request:{atomic:[{compose:'Order',targets:{ids:['3']},steps:[{create:'Note',values:{owner:{item:'owner'}}}]},{apply:'Order.confirm',target:{created:0}}]}}); console.log('target.created -> compose-created row',r.body.code,(r.body.msg||'').slice(0,70));
r=await raw('/apply',Hc,{key:k(),request:{atomic:[{create:'Order',values:{owner:'1',total:'1.00',status:'open'}}]}}); console.log('create ref to invisible Member',r.body.code,(r.body.msg||'').slice(0,70));
r=await raw('/apply',Hc,{key:k(),request:{atomic:[{create:'Order',values:{owner:'42',total:'1.00',status:'open'}},{apply:'Order.confirm',target:{ids:['1'],created:0}}]}}); console.log('target ids+created',r.body.code,(r.body.msg||'').slice(0,70));
// compose standalone with input step
r=await raw('/apply',Hc,{key:k(),request:{compose:'Order',targets:{ids:['3']},steps:[{transition:'confirm',input:{qty:1}}]}}); console.log('compose step input',r.body.code,(r.body.msg||'').slice(0,70));
