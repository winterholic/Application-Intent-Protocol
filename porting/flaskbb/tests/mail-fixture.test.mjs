import assert from 'node:assert/strict';
import {test} from 'node:test';
import net from 'node:net';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createServer} from 'node:http';
import {mailpitUrl, reserveRefusedPort, startMailpitProxy, stopOwnedProcess} from './mail-fixture.mjs';

test('Mailpit fixture URL must be explicit and local without credentials',()=>{
  assert.throws(()=>mailpitUrl({}),/is required/);
  for (const url of ['http://example.com:8025','http://127.0.0.1','http://127.0.0.1:80','http://user:secret@127.0.0.1:8025','http://127.0.0.1:8025/path','http://127.0.0.1:8025?query=1']) {
    assert.throws(()=>mailpitUrl({AIP_FLASKBB_TEST_MAILPIT_URL:url}),/loopback/);
  }
  assert.equal(mailpitUrl({AIP_FLASKBB_TEST_MAILPIT_URL:'http://127.0.0.1:55473/'}),'http://127.0.0.1:55473');
});

test('refused port stays owned while rejecting connects and is released on close',{skip:process.platform!=='linux'?'Linux CI outage fixture':false},async()=>{
  const reservation=await reserveRefusedPort();
  try {
    await assert.rejects(new Promise((resolve,reject)=>{
      const socket=net.connect(reservation.port,'127.0.0.1');
      socket.once('connect',()=>{socket.destroy();reject(new Error('unexpected listener'));});
      socket.once('error',reject);socket.setTimeout(2000,()=>{socket.destroy();reject(new Error('connect timeout'));});
    }),error=>error.code==='ECONNREFUSED');
    const competitor=net.createServer();
    await assert.rejects(new Promise((resolve,reject)=>{competitor.once('error',reject);competitor.listen(reservation.port,'127.0.0.1',resolve);}),error=>error.code==='EADDRINUSE');
  } finally {await reservation.close();}
  const released=net.createServer(socket=>socket.end());
  await new Promise((resolve,reject)=>{released.once('error',reject);released.listen(reservation.port,'127.0.0.1',resolve);});
  await new Promise(resolve=>released.close(resolve));
});

test('unsupported outage fixture fails before starting a helper',{skip:process.platform==='linux'},async()=>{
  await assert.rejects(reserveRefusedPort(),/requires Linux/);
});

test('recovered endpoint forwards only the declared send route and releases its listener',async()=>{
  let calls=0;
  const provider=createServer(async(req,res)=>{
    calls++;
    assert.equal(req.url,'/api/v1/send');
    const chunks=[];for await(const chunk of req)chunks.push(chunk);
    assert.deepEqual(JSON.parse(Buffer.concat(chunks)),{Subject:'fixture'});
    res.writeHead(201,{'Content-Type':'application/json'}).end('{"ID":"fixture-only"}');
  });
  await new Promise((resolve,reject)=>{provider.once('error',reject);provider.listen(0,'127.0.0.1',resolve);});
  let proxy;
  try {
    proxy=await startMailpitProxy({target:`http://127.0.0.1:${provider.address().port}`});
    assert.equal((await fetch(`${proxy.url}/unexpected`)).status,404);assert.equal(calls,0);
    const reply=await fetch(`${proxy.url}/api/v1/send`,{method:'POST',body:'{"Subject":"fixture"}'});
    assert.equal(reply.status,201);assert.deepEqual(await reply.json(),{ID:'fixture-only'});assert.equal(calls,1);
    await proxy.close();await proxy.close();
    await assert.rejects(fetch(`${proxy.url}/api/v1/send`,{method:'POST',signal:AbortSignal.timeout(2000)}));
  }finally {await proxy?.close();provider.closeAllConnections();await new Promise(resolve=>provider.close(resolve));}
});

test('owned server gets graceful SIGTERM and a stuck child is force-stopped',async()=>{
  for (const ignore of [false,true]) {
    const code=`process.on('SIGTERM',()=>{${ignore?'':'process.exit(0);'}});console.log('ready');setInterval(()=>{},1000);`;
    const child=spawn(process.execPath,['-e',code],{stdio:['ignore','pipe','ignore']});
    try {
      await once(child.stdout,'data');
      const [exit,signal]=await stopOwnedProcess(child,{graceMs:100,maxMs:5000});
      assert.equal(signal,ignore?'SIGKILL':null);assert.equal(exit,ignore?null:0);
      await stopOwnedProcess(child); // an already closed process is safe
    } finally {if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await once(child,'close');}}
  }
});
