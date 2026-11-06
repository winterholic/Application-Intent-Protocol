import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';

const app = readFileSync(new URL('../app.aip', import.meta.url), 'utf8');
const importSql = readFileSync(new URL('../seed/post_order_import.py', import.meta.url), 'utf8');
const upstream = readFileSync(new URL('../upstream/flaskbb/forum/models.py', import.meta.url), 'utf8');
const postStart = app.indexOf('resource Post {');
const postEnd = app.indexOf('resource PostOrder {', postStart);
const post = app.slice(postStart, postEnd);
const deleteStart = post.indexOf('transition delete {');
const deleteEnd = post.indexOf('expose apply delete', deleteStart);
const deletion = post.slice(deleteStart, deleteEnd);
const forumStart = deletion.indexOf('update Forum');
const topicStart = deletion.indexOf('update Topic');
const forumUpdate = deletion.slice(forumStart, topicStart);

test('pinned upstream delete predicates and each ordering source remain explicit', () => {
  const postDelete = upstream.slice(upstream.indexOf('    def delete(self):'), upstream.indexOf('    def hide(self, user):'));
  const dealStart = upstream.indexOf('    def _deal_with_last_post(self):');
  const dealEnd = upstream.indexOf('    def _update_counts(self):', dealStart);
  const deal = upstream.slice(dealStart, dealEnd);
  const topicStart = upstream.indexOf('class Topic(');
  const topicDeleteStart = upstream.indexOf('    def delete(self):', topicStart);
  const topicDeleteEnd = upstream.indexOf('    def hide(self, user):', topicDeleteStart);
  const topicDelete = upstream.slice(topicDeleteStart, topicDeleteEnd);
  const forumStart = upstream.indexOf('    def update_last_post(self, commit=True):');
  const forumEnd = upstream.indexOf('    def update_read(', forumStart);
  const forumRefresh = upstream.slice(forumStart, forumEnd);

  assert.match(postDelete, /if self\.topic\.first_post == self:[\s\S]*?self\.topic\.delete\(\)[\s\S]*?return self[\s\S]*?db\.session\.delete\(self\)[\s\S]*?self\._deal_with_last_post\(\)/);
  assert.match(deal, /if self\.topic\.last_post == self:[\s\S]*?if self\.topic\.last_post == self\.topic\.forum\.last_post:[\s\S]*?order_by\(\s*Post\.id\.desc\(\)\s*\)/);
  assert.match(topicDelete, /if forum\.last_post_id is None or[\s\\]*self\.last_post_id == forum\.last_post_id:[\s\S]*?forum\.update_last_post\(commit=False\)/);
  assert.match(forumRefresh, /order_by\(Post\.date_created\.desc\(\)\)/);
  assert.match(forumRefresh, /if last_post != self\.last_post:[\s\S]*?self\.last_post_created = last_post\.date_created[\s\S]*?else:[\s\S]*?self\.last_post_created = None/);
});

test('imported Forum fallback selects source ID 52 when date order selects 51', () => {
  const orderField = /order by candidate\.([a-z_]+)/.exec(forumUpdate)?.[1];
  assert.equal(orderField, 'source_order', 'Forum fallback must match upstream Post.id DESC via imported PostOrder');

  const remaining = [
    {sourceOrder: 51, date: '2026-01-02'},
    {sourceOrder: 52, date: '2026-01-01'},
  ];
  const orderBySourceId = [...remaining].sort((a, b) => b.sourceOrder - a.sourceOrder)[0];
  assert.equal(orderBySourceId.sourceOrder, 52);
  assert.equal(orderField === 'source_order' ? orderBySourceId.sourceOrder :
    [...remaining].sort((a, b) => b.date.localeCompare(a.date))[0].sourceOrder, 52);
});

test('Post.delete changes Forum projection only when the deleted Post is Forum.last_post', () => {
  assert.match(forumUpdate, /when[\s\S]*this\.forum\.last_post\s*=\s*this/,
    'upstream _deal_with_last_post preserves Forum fields unless the deleted post is also the Forum last post');
  for (const field of ['last_post', 'last_post_user', 'last_post_title', 'last_post_username', 'last_post_created']) {
    const assignment = forumUpdate.match(new RegExp(`^\\s+${field}\\s*=([^\\n]+)`, 'm'))?.[1] ?? '';
    assert.match(assignment, /otherwise null$/, `${field} must clear when no Forum replacement exists`);
  }
});

test('Post.delete preserves Forum fields unless both source pointers name the deleted Post', () => {
  assert.match(forumUpdate, /when\s+this\.topic\.last_post\s*=\s*this\s+and\s+this\.forum\.last_post\s*=\s*this/,
    'upstream enters Forum fallback only when both Topic.last_post and Forum.last_post equal the deleted Post');
});

test('Post.delete updates Topic only when the deleted Post is Topic.last_post', () => {
  assert.match(deletion, /update Topic when this\.topic\.last_post = this/);
  assert.match(deletion.slice(topicStart), /order by candidate\.source_order/);
});

test('PostOrder importer binds original Post IDs directly to source_order', () => {
  assert.match(importSql, /post_order\(id,post_id\)\s+OVERRIDING SYSTEM VALUE VALUES/);
  assert.match(importSql, /SET source_order_id=\{row\['source_id'\]\}/);
});

test('the legacy migration fixture removes the new nullable Topic pointer field including its delete policy', () => {
  const migrationTest = readFileSync(new URL('./post-order-migrate.test.mjs', import.meta.url), 'utf8');
  assert.match(migrationTest, /last_post: Post\? on delete set null/,
    'the preceding-schema fixture must anchor the active definition spelling');
});
