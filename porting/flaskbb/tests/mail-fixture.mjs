import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createInterface} from 'node:readline';

export async function stopOwnedProcess(child,{graceMs=8000,maxMs=10000}={}) {
  if (!child) return;
  if (child.exitCode!==null || child.signalCode!==null) return [child.exitCode,child.signalCode];
  const closed=once(child,'close');child.kill('SIGTERM');
  const force=setTimeout(()=>child.kill('SIGKILL'),graceMs);let timer;
  try {return await Promise.race([closed,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error('owned fixture process did not close')),maxMs);})]);}
  finally {clearTimeout(force);clearTimeout(timer);}
}

export function mailpitUrl(env=process.env) {
  const raw=env.AIP_FLASKBB_TEST_MAILPIT_URL;
  if (!raw) throw new Error('AIP_FLASKBB_TEST_MAILPIT_URL is required');
  const invalid=()=>new Error('Mailpit fixture requires an explicit loopback HTTP URL with a high port and no credentials or path');
  let url;try {url=new URL(raw);}catch {throw invalid();}
  if (raw.length>256 || url.protocol!=='http:' || !['localhost','127.0.0.1','[::1]'].includes(url.hostname)
      || !url.port || Number(url.port)<=1024 || url.username || url.password || url.pathname!=='/' || url.search || url.hash) throw invalid();
  return url.origin;
}

// Binding without listening both owns the TCP port and makes connect fail before a request can
// be sent. Dropping a temporary listener would let another process claim the supposedly dead port.
export async function reserveRefusedPort() {
  if (process.platform!=='linux') throw new Error('bound-port outage fixture requires Linux');
  const child=spawn('python3',['-I','-u','-c','import socket,sys\ns=socket.socket()\ns.bind(("127.0.0.1",0))\nprint(s.getsockname()[1],flush=True)\nsys.stdin.buffer.read()\ns.close()'],{
    env:{PATH:process.env.PATH},stdio:['pipe','pipe','pipe'],
  });
  let stderr='';child.stderr.on('data',data=>{stderr=(stderr+data).slice(-4096);});
  const lines=createInterface({input:child.stdout});let timer;
  let closing;
  const close=()=>closing??=(async()=>{
    if (child.exitCode!==null || child.signalCode!==null) {lines.close();return;}
    const exited=once(child,'exit');child.stdin.end();
    const kill=setTimeout(()=>child.kill('SIGKILL'),3000);
    let fail;
    try {await Promise.race([exited,new Promise((_,reject)=>{fail=setTimeout(()=>reject(new Error('refused-port helper did not stop')),5000);})]);}
    finally {clearTimeout(kill);clearTimeout(fail);lines.close();}
  })();
  try {
    const [line]=await Promise.race([
      once(lines,'line'),
      once(child,'error').then(([error])=>{throw error;}),
      once(child,'exit').then(()=>{throw new Error(`refused-port helper exited before readiness: ${stderr}`);}),
      new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error('refused-port helper readiness timeout')),5000);}),
    ]);
    const port=Number(line);if (!/^\d+$/.test(line) || port<=1024 || port>65535) throw new Error('refused-port helper returned an invalid port');
    return {port,url:`http://127.0.0.1:${port}`,close};
  } catch(error) {await close();throw error;}
  finally {clearTimeout(timer);}
}
