import fs from 'node:fs';
const TOKEN=fs.readFileSync('token.txt','utf8').trim();
const FP='bbd7980b8e1317f9a444f74616c7b782b1c7ce22eeda4a6819090f44a92079a5';
const H={'content-type':'application/json',authorization:'Bearer '+TOKEN,'x-aip-contract':FP};
for (const s of ['cnt','sm','mx','mn']) {
  const r=await (await fetch('http://127.0.0.1:18475/read',{method:'POST',headers:H,body:JSON.stringify({query:{read:'Member',select:['id',s]}})})).json();
  console.log(s, JSON.stringify(r.rows??r));
}
