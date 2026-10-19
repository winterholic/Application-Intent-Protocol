import assert from 'node:assert/strict';
import {test} from 'node:test';
import net from 'node:net';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {mailpitUrl, reserveRefusedPort, stopOwnedProcess} from './mail-fixture.mjs';

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
