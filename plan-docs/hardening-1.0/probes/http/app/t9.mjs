import fs from 'node:fs';
const TOKEN=fs.readFileSync('token.txt','utf8').trim();
const FP='5e7fa7d8d69eda48cf02f7d9406f5664bbc3058e550278d8f734806105b36bc0';
async function c(path,body,hdr){const r=await fetch('http://127.0.0.1:18471'+path,{method:'POST',headers:{'content-type':'application/json',authorization:'Bearer '+TOKEN,...hdr},body:JSON.stringify(body)});let t=await r.text();let j;try{j=JSON.parse(t)}catch{j=t.slice(0,80)};return r.status+' '+(j.code??JSON.stringify(j).slice(0,100))}
for (const p of ['/objects/sign','/objects/upload','/rag/search','/rag/index','/extension','/operation','/jobs/enqueue','/jobs/status','/files/put','/events/read','/messages/send','/session']) {
  console.log(p,'no-header:',await c(p,{}, {}),'| ok-header:',await c(p,{}, {'x-aip-contract':FP}));
}
console.log('GET capabilities',(await (await fetch('http://127.0.0.1:18471/capabilities')).text()).slice(0,120));
