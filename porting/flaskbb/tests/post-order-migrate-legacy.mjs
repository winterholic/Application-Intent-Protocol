import assert from 'node:assert/strict';

// Build the immediately preceding schema shape, restoring its Post pointer
// behavior before removing Topic.last_post and the unavailable PostOrder field.
export function deriveLegacyDefinition(source) {
  let legacy = source;
  const orderPredicates = legacy.match(/^predicate topicHide(?:OrderKnown|ReplacesForum|NullReplacement).*$/gm);
  assert.equal(orderPredicates?.length, 3);
  legacy = legacy.replace(/^predicate topicHide(?:OrderKnown|ReplacesForum|NullReplacement).*\n/gm, '');
  const deletePredicates = legacy.match(/^predicate topicDelete(?:Candidate|RecalculatesForum|KeepsForum).*$/gm);
  assert.equal(deletePredicates?.length, 3);
  legacy = legacy.replace(/^predicate topicDelete(?:Candidate|RecalculatesForum|KeepsForum).*\n/gm, '');
  const postPointerPredicates = legacy.match(/^predicate (?:postIsTopicLast|postIsForumLast|postRestoresTopicLast|topicLastCandidate|topicReplyBesides|topicHasRemainingReply).*$/gm);
  assert.equal(postPointerPredicates?.length, 6);
  legacy = legacy.replace(/^predicate (?:postIsTopicLast|postIsForumLast|postRestoresTopicLast|topicLastCandidate|topicReplyBesides|topicHasRemainingReply).*\n/gm, '');

  const deleteStart = legacy.indexOf('  transition delete {', legacy.indexOf('resource Topic {'));
  const deleteEnd = legacy.indexOf('  expose apply delete { target id; bulk maxRows 1 }', deleteStart);
  assert.ok(deleteStart > 0 && deleteEnd > deleteStart);
  legacy = legacy.slice(0, deleteStart) + '  transition delete { allow canDeleteTopic(actor, this); from deleted = false; to deleted = true }\n' + legacy.slice(deleteEnd);
  const hideStart = legacy.indexOf('  transition hide {', legacy.indexOf('resource Topic {'));
  const hideEnd = legacy.indexOf('  expose apply hide { target id; bulk maxRows 1 }', hideStart);
  assert.ok(hideStart > 0 && hideEnd > hideStart);
  legacy = legacy.slice(0, hideStart) + '  transition hide { allow canHide(actor, forum); from hidden != true and deleted = false; to hidden = true, hidden_at = now, hidden_by = actor }\n' + legacy.slice(hideEnd);

  const removeOnce = text => {
    assert.equal(legacy.split(text).length, 2, `legacy schema anchor must be unique: ${text}`);
    legacy = legacy.replace(text, '');
  };
  removeOnce('predicate postOrderKnownForForum(targetPost: Post) = not (exists Post where forum = targetPost.forum and published and not deleted and not topic.deleted and hidden != true and source_order = null)\n');
  assert.equal(legacy.split(' and postOrderKnownForForum(this)').length, 3);
  legacy = legacy.replaceAll(' and postOrderKnownForForum(this)', '');

  // Existing Forum projections contribute ten selectors. The G6 pointer
  // change adds one Topic fallback selector each to Post.hide and Post.delete.
  const sourceOrderSelectors = legacy.split('\n').filter(line => line.includes('order by candidate.source_order.id'));
  assert.equal(sourceOrderSelectors.filter(line => line.includes('candidate.forum = this.forum')).length, 10);
  assert.equal(sourceOrderSelectors.filter(line => line.includes('topicLastCandidate(this, candidate)')).length, 2);
  assert.equal(sourceOrderSelectors.length, 12);

  // The legacy schema predates Topic.last_post and the pre-state pointer
  // updates. Keep its former unconditional Forum projection and last_updated
  // recomputation so removing Topic.last_post leaves no dangling references.
  const forumPointerGuard = 'update Forum when postIsForumLast(this) where id = this.forum {';
  assert.equal(legacy.split(forumPointerGuard).length, 3);
  legacy = legacy.replaceAll(forumPointerGuard, 'update Forum where id = this.forum {');
  const legacyTopicDateUpdate = '    update Topic where id = this.topic {\n      last_updated = latest Post candidate where candidate.topic = this.topic and candidate.published = true and candidate.hidden = false and candidate.deleted = false order by candidate.date_created select candidate.date_created otherwise this.topic.date_created\n    }\n';
  const hidePointerUpdate = '    update Topic when postIsTopicLast(this) where id = this.topic {\n      last_post = latest Post candidate where topicLastCandidate(this, candidate) order by candidate.source_order.id select candidate.id otherwise this.topic.last_post\n      last_updated = this.date_created\n    }\n';
  assert.equal(legacy.split(hidePointerUpdate).length, 2);
  legacy = legacy.replace(hidePointerUpdate, legacyTopicDateUpdate);
  removeOnce('    update Topic when postRestoresTopicLast(this) where id = this.topic { last_post = this }\n');
  const deletePointerUpdate = '    update Topic when postIsTopicLast(this) and topicHasRemainingReply(this) where id = this.topic {\n      last_post = latest Post candidate where topicLastCandidate(this, candidate) order by candidate.source_order.id select candidate.id otherwise this.topic.last_post\n      last_updated = this.date_created\n    }\n';
  assert.equal(legacy.split(deletePointerUpdate).length, 2);
  legacy = legacy.replace(deletePointerUpdate, legacyTopicDateUpdate);
  removeOnce('    update Topic when postIsTopicLast(this) and not topicHasRemainingReply(this) where id = this.topic {\n      last_post = latest TopicFirstPost first where first.topic = this.topic order by first.id select first.post\n      last_updated = latest TopicFirstPost first where first.topic = this.topic order by first.id select first.post.date_created\n    }\n');
  const legacyOrderSelectors = legacy.split('\n').filter(line => line.includes('order by candidate.source_order.id'));
  assert.equal(legacyOrderSelectors.length, 10, 'legacy Forum projections retain their ten selectors; new Topic selectors are removed with the new pointer updates');
  legacy = legacy.replaceAll('order by candidate.source_order.id', 'order by candidate.date_created');

  removeOnce('    source_order: PostOrder?\n');
  // This fixture preserves the prior schema, without guessing a historical Topic pointer.
  const topicFieldAnchor = 'resource Topic {\n  fields {\n    id: Uuid\n    last_post: Post?\n';
  assert.equal(legacy.split(topicFieldAnchor).length, 2);
  legacy = legacy.replace(topicFieldAnchor, 'resource Topic {\n  fields {\n    id: Uuid\n');
  removeOnce('    update Topic where id = this.topic { last_post = this; last_updated = now }\n');
  legacy = legacy.replace('last_post = this.last_post\n', 'last_post = this.forum.last_post\n');
  removeOnce('  unique source_order\n  check sourceOrderBelongsToPost when source_order = null or source_order.post = this\n');
  removeOnce(' and not (exists Post where published = true and source_order = null)');
  removeOnce('    create PostOrder { post = this }\n    update Post where id = this { source_order = latest PostOrder mapped where mapped.post = this order by mapped.id select mapped.id }\n');
  removeOnce('resource PostOrder {\n  fields { id: Id; post: Post }\n  unique post\n  rows read when canReadPost(actor, post)\n}\n');
  assert.doesNotMatch(legacy, /\bsource_order\b|sourceOrderBelongsToPost|\bPostOrder\b/, 'legacy fixture must not reference removed PostOrder fields or resource');
  return legacy;
}
