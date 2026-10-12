//! The same definition and the same request give the same SQL, parameters and DDL. A change of text that
//! does not change meaning (declaration order, member order, comments) gives the same contract digest.
//!
//! Every `HashMap` instance gets its own random seed, so loading and planning many times in one process would
//! show an iteration order that leaks into an output. The CLI-level check across processes is in
//! `plan-docs/reviews/2026-10-13-ai-friendliness-runtime-reliability-audit.md` (F12).
use serde_json::{json, Value};
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::sqlgen;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";

fn facts_of(src: &str) -> Value {
    load_str(src, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

fn requests() -> Vec<Value> {
    vec![
        json!({
            "read": "Recruitment",
            "select": ["id", "title", "periodEnd", "bookmarkCount", "internalNote", { "club": { "select": ["id", "name", "logo"] } }],
            "sort": [{ "field": "periodEnd", "dir": "asc" }],
            "limit": 20
        }),
        json!({
            "read": "Recruitment",
            "select": ["id", "views"],
            "filter": [{ "field": "periodEnd", "op": "gte", "value": NOW }, { "field": "periodEnd", "op": "lte", "value": NOW }],
            "sort": [{ "field": "views", "dir": "desc" }, { "field": "id", "dir": "asc" }],
            "limit": 5
        }),
    ]
}

/// Everything a plan says, in one string: SQL, bound values, cost and the tables it reads.
fn plans(facts: &Value) -> String {
    let caller = Caller { actor_id: Some(7), now: NOW.into() };
    requests()
        .iter()
        .map(|req| {
            let p = plan_read(facts, req, &caller).unwrap_or_else(|e| panic!("{e:?}"));
            format!("{}\n{:?}\n{} {} {:?}", p.sql, p.params, p.cost, p.deadline_ms, p.deps)
        })
        .collect::<Vec<_>>()
        .join("\n--\n")
}

fn ddl(facts: &Value) -> String {
    sqlgen::ddl(facts).unwrap_or_else(|e| panic!("{e:?}")).join("\n")
}

/// Top-level declarations: a line at column 0 starts one, and the comment lines right before it belong to it.
fn declarations(src: &str) -> Vec<String> {
    let mut blocks: Vec<Vec<&str>> = vec![vec![]];
    for line in src.lines() {
        let starts = !line.is_empty() && !line.starts_with(char::is_whitespace) && !line.starts_with('}') && !line.starts_with("//");
        if starts && blocks.last().is_some_and(|b| b.iter().any(|l| !l.starts_with("//") && !l.is_empty())) {
            let mut carried = Vec::new();
            while let Some(last) = blocks.last_mut().and_then(|b| b.last().copied().filter(|l| l.starts_with("//"))) {
                blocks.last_mut().expect("block").pop();
                carried.insert(0, last);
            }
            blocks.push(carried);
        }
        blocks.last_mut().expect("block").push(line);
    }
    blocks.into_iter().map(|b| b.join("\n")).collect()
}

/// A fixed sequence of permutations: reversal, rotations and a seeded shuffle, so a failure reproduces.
fn permutations(n: usize) -> Vec<Vec<usize>> {
    let mut out = vec![(0..n).rev().collect::<Vec<_>>()];
    out.extend((1..n).step_by(3).map(|k| (0..n).map(|i| (i + k) % n).collect()));
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..6 {
        let mut order: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            order.swap(i, (state % (i as u64 + 1)) as usize);
        }
        out.push(order);
    }
    out
}

#[test]
fn many_loads_in_one_process_give_identical_digests_plans_and_ddl() {
    let first = facts_of(A);
    let (digest0, plans0, ddl0) = (digest(&first), plans(&first), ddl(&first));
    for round in 0..40 {
        let again = facts_of(A);
        assert_eq!(digest(&again), digest0, "digest, round {round}");
        assert_eq!(plans(&again), plans0, "plans, round {round}");
        assert_eq!(ddl(&again), ddl0, "ddl, round {round}");
    }
}

#[test]
fn declaration_order_changes_neither_the_digest_nor_the_sql() {
    let blocks = declarations(A);
    assert!(blocks.len() >= 12, "the fixture has many declarations: {}", blocks.len());
    let base = facts_of(A);
    let (digest0, plans0, ddl0) = (digest(&base), plans(&base), ddl(&base));
    for order in permutations(blocks.len()) {
        let shuffled: String = order.iter().map(|i| blocks[*i].as_str()).collect::<Vec<_>>().join("\n");
        let facts = facts_of(&shuffled);
        assert_eq!(digest(&facts), digest0, "digest differs for order {order:?}");
        assert_eq!(plans(&facts), plans0, "plans differ for order {order:?}");
        assert_eq!(ddl(&facts), ddl0, "ddl differs for order {order:?}");
    }
}

#[test]
fn member_order_comments_and_spacing_do_not_change_the_digest() {
    let digest0 = digest(&facts_of(A));
    let variants = [
        ("field order", "    title: Text(1..100)\n    periodEnd: Time\n", "    periodEnd: Time\n    title: Text(1..100)\n"),
        (
            "member order",
            "  rows read when active(this) and\n    (club.school = null or club.school = actor.school)\n  field internalNote read when managerOf(actor, club)\n",
            "  field internalNote read when managerOf(actor, club)\n  rows read when active(this) and\n    (club.school = null or club.school = actor.school)\n",
        ),
        ("select order", "select id, title, periodEnd, views, bookmarkCount, internalNote", "select internalNote, bookmarkCount, views, periodEnd, title, id"),
        ("filter order", "filter periodEnd.gte, periodEnd.lte", "filter periodEnd.lte, periodEnd.gte"),
        ("comments and blank lines", "// V1", "// a different comment\n\n\n// V1"),
    ];
    for (what, old, new) in variants {
        assert_eq!(A.matches(old).count(), 1, "{what}: the sample needs `{old}` once");
        assert_eq!(digest(&facts_of(&A.replacen(old, new, 1))), digest0, "{what} must not change the digest");
    }
}

/// Recorded, not endorsed: the order of an enum's values is part of the contract digest, although it is stored as a
/// `CHECK (col IN (...))` where order means nothing. A reorder is therefore a contract change (a new digest, a
/// migration plan) with no change of behavior. See the audit, F12.
#[test]
fn enum_value_order_is_part_of_the_digest() {
    let reordered = A.replacen("enum RecruitmentStatus { DRAFT, PUBLISHED, CLOSED }", "enum RecruitmentStatus { CLOSED, PUBLISHED, DRAFT }", 1);
    assert_ne!(digest(&facts_of(&reordered)), digest(&facts_of(A)));
}
