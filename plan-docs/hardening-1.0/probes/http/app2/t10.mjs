import fs from 'node:fs';
const TOKEN=fs.readFileSync('token.txt','utf8').trim();
const FP='d7f517e53bc41730a771efd5fb2546d7bfe006c29e8540e026c636b4ad023955';
const H={'content-type':'application/json',authorization:'Bearer '+TOKEN,'x-aip-contract':FP};
for (const n of [50,51]) {
  const r=await (await fetch('http://127.0.0.1:18472/read',{method:'POST',headers:H,body:JSON.stringify({query:{read:'Item',select:['id'],filter:[{field:'id',op:'in',value:Array.from({length:n},(_,i)=>String(i+1))}]}})})).json();
  console.log('in',n,r.code??'ok rows='+r.rows.length, (r.msg||'').slice(0,50));
}
const r=await (await fetch('http://127.0.0.1:18472/read',{method:'POST',headers:H,body:JSON.stringify({query:{read:'Item',select:['id','name'],sort:[{field:'name',dir:'asc'}],limit:3,offset:1}})})).json();
console.log('offset',r.code??r.rows.map(x=>x.name));
const r2=await (await fetch('http://127.0.0.1:18472/read',{method:'POST',headers:H,body:JSON.stringify({query:{read:'Item',select:['id'],offset:101}})})).json();
console.log('offset 101 (>100)',r2.code,(r2.msg||'').slice(0,50));
