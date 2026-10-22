import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
import {resolve} from 'node:path';

const [base, bindingPath, sdkEntry] = process.argv.slice(2);
assert.ok(base && bindingPath && sdkEntry, 'usage: node --experimental-strip-types facereview-member-sdk-journey.mjs <base> <generated-binding.ts> <sdk-index.js>');

const origin = 'http://localhost:5173';
const email = process.env.AIP_FACEREVIEW_JOURNEY_EMAIL;
const password = process.env.AIP_FACEREVIEW_JOURNEY_PASSWORD;
assert.ok(email && password, 'journey credentials must come from the dedicated fixture environment');

const auth = async (path, payload, cookie) => fetch(`${base}${path}`, {
  method: 'POST',
  headers: {
    'content-type': 'application/json',
    origin,
    ...(cookie ? {cookie} : {}),
  },
  body: JSON.stringify(payload),
});
const refreshCookie = response => {
  const value = response.headers.getSetCookie().find(header => header.startsWith('refresh_token='));
  assert.ok(value, `expected refresh cookie from ${response.status}`);
  return value.split(';', 1)[0];
};
const accessToken = async response => {
  const body = await response.json();
  assert.equal(response.ok, true, `auth response status ${response.status}: ${body.code ?? body.reason ?? 'unknown'}`);
  assert.equal(typeof body.access_token, 'string');
  return body.access_token;
};

const login = await auth('/auth/login', {email, password});
assert.equal(login.status, 200, 'fixture member login');
let cookie = refreshCookie(login);
let token = await accessToken(login);
const subject = JSON.parse(Buffer.from(token.split('.')[1], 'base64url').toString()).sub;
assert.equal(typeof subject, 'string');

const reissue = await auth('/auth/reissue', {}, cookie);
assert.equal(reissue.status, 200, 'single refresh rotates the cookie');
cookie = refreshCookie(reissue);
token = await accessToken(reissue);

const sdk = await import(pathToFileURL(sdkEntry).href);
const {contract} = await import(pathToFileURL(bindingPath).href);
const {createReadApi} = await import(pathToFileURL(resolve(import.meta.dirname, '../../porting/facereview/ui-overlay/src/api/readAdapter.ts')).href);
const {createWriteApi} = await import(pathToFileURL(resolve(import.meta.dirname, '../../porting/facereview/ui-overlay/src/api/writeAdapter.ts')).href);
const client = sdk.connect(base, token, contract);
const reads = createReadApi(client, {newKey: () => crypto.randomUUID(), warn: () => {}});
const writes = createWriteApi(client, {userId: () => subject, newKey: () => crypto.randomUUID()});

const videos = await reads.getAllVideo({page: 1, size: 10, emotion: 'all'});
const video = videos.find(item => item.video_id === 'aaaaaaaa-0000-4000-8000-000000000001');
assert.ok(video, 'the authenticated UI read adapter returns the seeded video');

assert.deepEqual(await writes.toggleBookmark({video_id: video.video_id}), {
  is_bookmarked: true,
  message: '북마크에 추가했어요.',
});
assert.deepEqual(await writes.toggleBookmark({video_id: video.video_id}), {
  is_bookmarked: false,
  message: '북마크를 해제했어요.',
});

const comment = await writes.sendNewComment({video_id: video.video_id, content: 'AIP member journey fixture'});
assert.ok(comment.comment_id);
await writes.modifyComment({comment_id: comment.comment_id, content: 'AIP member journey edited'});
const edited = await client.read({
  read: 'Comment', select: ['id', 'content', 'is_deleted'],
  filter: [{field: 'id', op: 'eq', value: comment.comment_id}], limit: 1,
});
assert.equal(edited.rows[0]?.content, 'AIP member journey edited');
await writes.deleteComment({comment_id: comment.comment_id});
const deleted = await client.read({
  read: 'Comment', select: ['id', 'is_deleted'],
  filter: [{field: 'id', op: 'eq', value: comment.comment_id}], limit: 1,
});
assert.equal(deleted.rows.length, 0, 'the member row policy hides a soft-deleted comment');

const logout = await auth('/auth/logout', {}, cookie);
assert.equal(logout.status, 200, 'logout revokes the member session');
const stale = await fetch(`${base}/session`, {
  method: 'POST', headers: {'content-type': 'application/json', authorization: `Bearer ${token}`}, body: '{}',
});
const staleBody = await stale.json();
assert.equal(stale.status, 200, 'session failures use the AIP response envelope');
assert.equal(staleBody.ok, false, 'logout invalidates the access token');
assert.equal(staleBody.code, 'WIRE.UNAUTHENTICATED', 'logout invalidates the access token');

console.log('member SDK journey: login, one refresh, UI adapter video read, bookmark on/off, comment create/edit/delete, logout');
