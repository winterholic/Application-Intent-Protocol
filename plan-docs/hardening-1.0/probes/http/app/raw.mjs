import fs from 'node:fs';
const TOKEN=fs.readFileSync('token.txt','utf8').trim();
const FP='5e7fa7d8d69eda48cf02f7d9406f5664bbc3058e550278d8f734806105b36bc0';
export async function post(path, body, {key, token=TOKEN, fp=FP}={}){
  const headers={'content-type':'application/json','x-aip-contract':fp};
  if(token) headers.authorization='Bearer '+token;
  const b = path==='/apply' ? {key, request: body} : body;
  const r=await fetch('http://127.0.0.1:18471'+path,{method:'POST',headers,body:JSON.stringify(b)});
  return {status:r.status, body: await r.json()};
}
export const show=(l,x)=>console.log(l, JSON.stringify(x.body ?? x));
