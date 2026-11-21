"""콘텐츠·커뮤니티 시나리오(Python SDK). 상태 전이, 관계·집계, cursor/offset 페이지를 생성 helper로 확인한다."""
from aip_sdk import connect
from binding import (CONTRACT_FINGERPRINT, apply_post_hide, apply_post_publish, apply_post_restore, apply_post_view,
                     create_comment, create_post, create_reaction, read_board, read_comment, read_post)
from check import BASE_URL, SCHEMA, TOKENS, scenario, sql


def client(who):
    return connect(BASE_URL, TOKENS[who] if who else None, CONTRACT_FINGERPRINT)


mia, noah, mod = client("mia"), client("noah"), client("mod")
posts = []


def ok(result, what):
    assert result["ok"] is True, f"{what}: {result}"
    return result


def denied(result, code, what):
    assert result["ok"] is False and result["code"] == code, f"{what}: {result}"


def t_lifecycle():
    for i in range(6):
        made = ok(create_post(mia, {"board": "1" if i < 4 else "2", "author": "1", "title": f"post {i}", "body": "b", "status": "DRAFT", "views": 0, "version": 0}, key=f"py-post-{i}"), "create")
        posts.append(made["created"][0]["id"])
    assert read_post(client("noah"), {"read": "Post", "select": ["id"], "limit": 20}) == [], "draft hidden from readers"
    assert len(read_post(mia, {"read": "Post", "select": ["id"], "limit": 20})) == 6
    denied(apply_post_publish(noah, {"ids": [posts[0]]}, key="py-noah-pub"), "WIRE.MISSING_TARGET", "foreign draft")
    ok(apply_post_publish(mia, {"ids": posts[:5]}, key="py-publish"), "bulk publish")
    assert len(read_post(client("noah"), {"read": "Post", "select": ["id"], "limit": 20})) == 5
    assert len(read_post(client(None), {"read": "Post", "select": ["id"], "limit": 20})) == 5, "anonymous reads published"
    assert apply_post_publish(mia, {"ids": [posts[0]]}, key="py-publish-again")["ok"] is False
    denied(apply_post_hide(noah, {"ids": [posts[1]]}, key="py-noah-hide"), "WIRE.FORBIDDEN", "reader hide")
    ok(apply_post_hide(mod, {"ids": [posts[1]]}, key="py-mod-hide"), "mod hide")
    assert len(read_post(client("noah"), {"read": "Post", "select": ["id"], "limit": 20})) == 4
    assert read_post(mia, {"read": "Post", "select": ["id", "status"], "filter": [{"field": "id", "op": "eq", "value": posts[1]}]})[0]["status"] == "HIDDEN"
    ok(apply_post_restore(mod, {"ids": [posts[1]]}, key="py-mod-restore"), "restore")
    assert sql(f"SELECT status||'|'||version FROM {SCHEMA}.post WHERE id={posts[1]}") == "PUBLISHED|3"


def t_views_cursor():
    for n, i in ((3, 2), (2, 3), (2, 4)):
        for k in range(n):
            ok(apply_post_view(noah, {"ids": [posts[i]]}, key=f"py-view-{i}-{k}"), "view")
    seen, after = [], None
    for _ in range(10):
        q = {"read": "Post", "select": ["id", "views"], "sort": [{"field": "views", "dir": "desc"}], "limit": 2}
        if after:
            q["after"] = after
        rows = read_post(noah, q)
        if not rows:
            break
        seen += [r["id"] for r in rows]
        after = {"views": rows[-1]["views"], "id": rows[-1]["id"]}
        if len(rows) < 2:
            break
    expected = sql(f"SELECT string_agg(id::text, ',' ORDER BY views DESC, id ASC) FROM {SCHEMA}.post WHERE status='PUBLISHED'").split(",")
    assert seen == expected and len(seen) == 5, (seen, expected)
    filtered = read_post(noah, {"read": "Post", "select": ["id", "views"], "filter": [{"field": "board", "op": "eq", "value": "1"}, {"field": "views", "op": "gte", "value": 2}], "sort": [{"field": "views", "dir": "desc"}]})
    assert [r["views"] for r in filtered] == [3, 2], filtered


def t_comments_aggregates_offset():
    p = posts[0]
    for i in range(7):
        ok(create_comment(noah, {"post": p, "author": "2", "body": f"c{i}", "hidden": False}, key=f"py-c{i}"), "comment")
    denied(create_comment(noah, {"post": posts[5], "author": "2", "body": "on draft", "hidden": False}, key="py-c-draft"), "WIRE.MISSING_TARGET", "draft comment")
    bodies = []
    for off in (0, 3, 6):
        bodies += [r["body"] for r in read_comment(noah, {"read": "Comment", "select": ["id", "body"], "filter": [{"field": "post", "op": "eq", "value": p}], "sort": [{"field": "id", "dir": "asc"}], "limit": 3, "offset": off})]
    assert bodies == [f"c{i}" for i in range(7)], bodies
    row = read_post(noah, {"read": "Post", "select": ["id", "commentCount", {"author": {"select": ["id", "name"]}}, {"comments": {"select": ["id"], "limit": 3}}], "filter": [{"field": "id", "op": "eq", "value": p}]})[0]
    assert row["commentCount"] == 7 and row["author"] == {"id": "1", "name": "Mia"} and len(row["comments"]) == 3
    ok(create_reaction(mia, {"post": p, "member": "1", "score": 5}, key="py-r1"), "r1")
    ok(create_reaction(noah, {"post": p, "member": "2", "score": 3}, key="py-r2"), "r2")
    denied(create_reaction(noah, {"post": p, "member": "2", "score": 4}, key="py-r3"), "WIRE.ALREADY_EXISTS", "unique reaction")
    r = read_post(client("noah"), {"read": "Post", "select": ["id", "reactionCount", "scoreSum"], "filter": [{"field": "id", "op": "eq", "value": p}]})[0]
    assert r["reactionCount"] == 2 and r["scoreSum"] == 3, r
    boards = read_board(client(None), {"read": "Board", "select": ["id", "publishedCount"], "sort": [{"field": "id", "dir": "asc"}]})
    assert [b["publishedCount"] for b in boards] == [4, 1], boards


scenario("draft-publish-hide-restore-lifecycle", t_lifecycle)
scenario("views-counter-and-cursor-feed", t_views_cursor)
scenario("comments-offset-aggregates-count-vs-sum", t_comments_aggregates_offset)
