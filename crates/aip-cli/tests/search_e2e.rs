//! End-to-end for `search`: full-text search over the SaaS example's tasks (English, stemmed) and projects (Korean, by word prefix).
//!
//! What has to hold: a title hit outranks a note hit, several words narrow the result, a query with no searchable word
//! finds nothing instead of everything, rows the caller may not see and rows of another tenant are in no result,
//! and paging by cursor neither repeats nor skips a row while rows are added. `search_negative_controls` switches each
//! of those checks off in the plan and requires the matching probe to fail, so these assertions can fail.

mod common;

use aip_runtime::engine::Engine;
use common::{call, fails, ok, sql_one};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static KEY: AtomicUsize = AtomicUsize::new(0);

fn key() -> String {
    format!("s{}", KEY.fetch_add(1, Ordering::Relaxed))
}

fn id(v: &Value) -> String {
    v["id"].as_str().expect("id").to_string()
}

async fn member(e: &Engine, name: &str) -> String {
    sql_one(e, &format!("INSERT INTO member (email) VALUES ('{name}@x.com') RETURNING id")).await.as_str().expect("id").to_string()
}

async fn workspace(e: &Engine, owner: &str, name: &str) -> String {
    id(&ok(e, "CreateWorkspace", Some(owner), json!({"name": name}), Some(&key())).await)
}

async fn project(e: &Engine, actor: &str, ws: &str, title: &str) -> String {
    id(&ok(e, "CreateProject", Some(actor), json!({"workspace": ws, "title": title}), Some(&key())).await)
}

async fn task(e: &Engine, actor: &str, project: &str, title: &str, notes: Option<&str>) -> String {
    let mut input = json!({"project": project, "title": title});
    if let Some(n) = notes {
        input["notes"] = json!(n);
    }
    id(&ok(e, "CreateTask", Some(actor), input, Some(&key())).await)
}

/// alice is only in workspace A, bob only in B, carol in both and dave in neither.
struct World {
    e: Arc<Engine>,
    alice: String,
    bob: String,
    carol: String,
    dave: String,
    wa: String,
    wb: String,
    pa: String,
    pb: String,
}

async fn world(e: Arc<Engine>) -> World {
    let (alice, bob, carol, dave) = (member(&e, "alice").await, member(&e, "bob").await, member(&e, "carol").await, member(&e, "dave").await);
    let wa = workspace(&e, &alice, "A").await;
    let wb = workspace(&e, &bob, "B").await;
    ok(&e, "AddMember", Some(&alice), json!({"workspace": wa, "member": carol}), None).await;
    ok(&e, "AddMember", Some(&bob), json!({"workspace": wb, "member": carol}), None).await;
    let pa = project(&e, &alice, &wa, "프로그래밍 스터디").await;
    let pb = project(&e, &bob, &wb, "프로그래밍 동아리").await;
    World { e, alice, bob, carol, dave, wa, wb, pa, pb }
}

async fn search(e: &Engine, actor: &str, ws: &str, q: &str) -> Vec<Value> {
    ok(e, "SearchTasks", Some(actor), json!({"workspace": ws, "q": q}), None).await.as_array().cloned().unwrap_or_default()
}

fn titles(rows: &[Value]) -> Vec<String> {
    rows.iter().map(|r| r["title"].as_str().unwrap_or_default().to_string()).collect()
}

fn rank(v: &Value) -> f64 {
    v["rank"].as_str().and_then(|s| s.parse().ok()).expect("rank is a decimal string")
}

/// Everything a probe learns about one deployment of the example.
struct Probes {
    /// title hit and note hit for the same word, best first, with their ranks
    weighted: Vec<(String, f64)>,
    both_words: Vec<String>,
    either_word: Vec<String>,
    phrase: Vec<String>,
    excluded: Vec<String>,
    stemmed: Vec<String>,
    /// carol is in both workspaces and asks A
    carol_in_a: Vec<String>,
    carol_in_b: Vec<String>,
    /// dave belongs to no workspace and asks A
    stranger: Vec<String>,
    stranger_has_more: bool,
    /// the rows of one workspace paged through while three more matching rows appear after the first page
    paged: Vec<String>,
    paged_expected: BTreeSet<String>,
    page_sizes: Vec<usize>,
}

async fn probes(w: &World) -> Probes {
    let e = &w.e;
    // weights: the same word in a title (A) and in notes (B)
    task(e, &w.alice, &w.pa, "invoice migration", None).await;
    task(e, &w.alice, &w.pa, "cleanup", Some("invoice migration plan")).await;
    task(e, &w.alice, &w.pa, "invoice cleanup", None).await;
    task(e, &w.alice, &w.pa, "quarterly migration", None).await;
    // a task of the other tenant that matches as well
    task(e, &w.bob, &w.pb, "invoice migration of B", None).await;

    let rows = search(e, &w.alice, &w.wa, "migration").await;
    let weighted = rows.iter().take(2).map(|r| (r["title"].as_str().unwrap_or_default().to_string(), rank(r))).collect();
    let both_words = titles(&search(e, &w.alice, &w.wa, "invoice migration").await);
    let either_word = titles(&search(e, &w.alice, &w.wa, "cleanup or quarterly").await);
    let phrase = titles(&search(e, &w.alice, &w.wa, "\"invoice migration\"").await);
    let excluded = titles(&search(e, &w.alice, &w.wa, "invoice -migration").await);
    let stemmed = titles(&search(e, &w.alice, &w.wa, "migrations").await);
    let carol_in_a = titles(&search(e, &w.carol, &w.wa, "invoice").await);
    let carol_in_b = titles(&search(e, &w.carol, &w.wb, "invoice").await);
    let stranger_reply =
        call(e, "SearchTasks", Some(&w.dave), json!({"workspace": w.wa, "q": "invoice"}), None).await.expect("a stranger is answered");
    let stranger = titles(stranger_reply.data.as_array().map(Vec::as_slice).unwrap_or_default());
    let stranger_has_more = stranger_reply.page.as_ref().and_then(|p| p["has_more"].as_bool()).unwrap_or(true);

    // paging: 25 rows with the same score, so the id has to break every tie
    let mut expected = BTreeSet::new();
    for n in 0..25 {
        expected.insert(task(e, &w.alice, &w.pa, &format!("report {n:02}"), None).await);
    }
    let mut seen: Vec<String> = Vec::new();
    let mut page_sizes = Vec::new();
    let mut cursor: Option<String> = None;
    let mut first = true;
    loop {
        let mut input = json!({"workspace": w.wa, "q": "report"});
        if let Some(c) = &cursor {
            input["cursor"] = json!(c);
        }
        let r = call(e, "SearchTasks", Some(&w.alice), input, None).await.expect("page");
        let rows = r.data.as_array().cloned().unwrap_or_default();
        page_sizes.push(rows.len());
        seen.extend(rows.iter().map(id));
        if first {
            first = false;
            // rows arrive between the pages: equal scores (their ids fall on either side of the cursor) and a better one
            for n in 0..3 {
                task(e, &w.alice, &w.pa, &format!("report {n:02} again"), None).await;
            }
            task(e, &w.alice, &w.pa, "report report report", None).await;
        }
        cursor = r.page.as_ref().and_then(|p| p["next_cursor"].as_str()).map(String::from);
        if cursor.is_none() {
            break;
        }
    }
    Probes {
        weighted,
        both_words,
        either_word,
        phrase,
        excluded,
        stemmed,
        carol_in_a,
        carol_in_b,
        stranger,
        stranger_has_more,
        paged: seen,
        paged_expected: expected,
        page_sizes,
    }
}

async fn found_projects(w: &World, q: &str) -> Vec<String> {
    ok(&w.e, "SearchProjects", Some(&w.alice), json!({"workspace": w.wa, "q": q}), None)
        .await
        .as_array()
        .map(|a| a.iter().map(|r| r["title"].as_str().unwrap_or_default().to_string()).collect::<Vec<_>>())
        .unwrap_or_default()
}

fn set(v: &[String]) -> BTreeSet<&str> {
    v.iter().map(String::as_str).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_end_to_end() {
    let e = common::setup_app("saas", "aip_e2e_search").await;
    let w = world(e).await;
    let p = probes(&w).await;
    let e = &w.e;

    // --- weights: the title hit comes before the note hit and scores higher
    // the two title hits tie, and the id orders them
    let top: BTreeSet<&str> = p.weighted.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(top, BTreeSet::from(["invoice migration", "quarterly migration"]), "{:?}", p.weighted);
    assert!(p.weighted[0].1 > 0.0);
    let ranks = search(e, &w.alice, &w.wa, "migration").await;
    assert_eq!(ranks.len(), 3, "title, title and the note: {ranks:?}");
    assert!(rank(&ranks[0]) >= rank(&ranks[1]) && rank(&ranks[1]) > rank(&ranks[2]), "best match first: {ranks:?}");
    assert_eq!(ranks[2]["title"], json!("cleanup"), "a note hit scores below any title hit");
    assert!(ranks[0]["rank"].is_string(), "a score is a decimal, which crosses the wire as a string");

    // --- several words: all of them, any of them, a phrase, a word to leave out, a word in another form
    assert_eq!(set(&p.both_words), set(&["invoice migration".to_string(), "cleanup".to_string()]), "{:?}", p.both_words);
    assert_eq!(set(&p.either_word), set(&["cleanup".to_string(), "invoice cleanup".to_string(), "quarterly migration".to_string()]));
    assert_eq!(
        set(&p.phrase),
        set(&["invoice migration".to_string(), "cleanup".to_string()]),
        "the phrase is in a title and in the notes of another task: {:?}",
        p.phrase
    );
    assert_eq!(p.excluded, ["invoice cleanup"], "{:?}", p.excluded);
    assert_eq!(p.stemmed.len(), 3, "'migrations' finds 'migration' in English: {:?}", p.stemmed);

    // --- a query with nothing to search for: below the length limit it is invalid, with only punctuation or a stop word it finds nothing
    for q in ["", "   "] {
        let err = fails(e, "SearchTasks", Some(&w.alice), json!({"workspace": w.wa, "q": q}), None).await;
        assert_eq!(err.code, "AIP.INPUT.INVALID", "{q:?}: {err}");
    }
    for q in ["!!!", "the", "- \"\""] {
        assert!(search(e, &w.alice, &w.wa, q).await.is_empty(), "{q:?} has no searchable word, so it finds nothing (not everything)");
    }

    // --- rows the caller may not see, and rows of the other tenant, are in no result
    assert_eq!(
        set(&p.carol_in_a),
        set(&["invoice migration".to_string(), "invoice cleanup".to_string(), "cleanup".to_string()]),
        "{:?}",
        p.carol_in_a
    );
    assert_eq!(p.carol_in_b, ["invoice migration of B"], "someone in both workspaces gets only the one asked about: {:?}", p.carol_in_b);
    assert!(p.stranger.is_empty() && !p.stranger_has_more, "a stranger finds nothing, and the page does not say there is more: {:?}", p.stranger);
    let bobs = search(e, &w.bob, &w.wa, "invoice").await;
    assert!(bobs.is_empty(), "a member of B asking about A's workspace sees none of A's rows: {bobs:?}");
    let alice_b = search(e, &w.alice, &w.wb, "invoice").await;
    assert!(alice_b.is_empty(), "and the other way round: {alice_b:?}");

    // --- the index follows the rows: a changed title is found under its new words only
    sql_one(e, &format!("UPDATE task SET title = 'renamed budget' WHERE title = 'quarterly migration' AND project_id = '{}' RETURNING id", w.pa))
        .await;
    assert!(search(e, &w.alice, &w.wa, "quarterly").await.is_empty());
    assert_eq!(titles(&search(e, &w.alice, &w.wa, "budget").await), ["renamed budget"]);

    // --- paging by cursor while rows are added
    assert_eq!(p.page_sizes[0], 20, "{:?}", p.page_sizes);
    let seen: BTreeSet<&str> = p.paged.iter().map(String::as_str).collect();
    assert_eq!(seen.len(), p.paged.len(), "no row twice: {:?}", p.page_sizes);
    for id in &p.paged_expected {
        assert!(seen.contains(id.as_str()), "{id} was on no page");
    }
    assert!(p.paged.len() >= 25 && p.paged.len() <= 28, "rows added after the cursor are shown, those before it are not: {}", p.paged.len());

    // --- Korean: words match by prefix (not by stem), offset paging, and the contract says so
    ok(e, "CreateProject", Some(&w.alice), json!({"workspace": w.wa, "title": "자바개발자 모집"}), Some(&key())).await;
    ok(e, "CreateProject", Some(&w.alice), json!({"workspace": w.wa, "title": "프로그래밍을 공부하는 모임"}), Some(&key())).await;
    let found = |q: &'static str| found_projects(&w, q);
    let by_prefix = found("프로그래밍").await;
    assert_eq!(
        set(&by_prefix),
        set(&["프로그래밍 스터디".to_string(), "프로그래밍을 공부하는 모임".to_string()]),
        "an ending after the stem is tolerated: {by_prefix:?}"
    );
    assert!(found("개발").await.is_empty(), "a word in the middle of a compound is not found: this is a prefix match, not morphological analysis");
    assert_eq!(found("자바").await, ["자바개발자 모집"]);
    assert!(found("프로그래밍 개발").await.is_empty(), "words are ANDed");
    let other_tenant = ok(e, "SearchProjects", Some(&w.carol), json!({"workspace": w.wb, "q": "프로그래밍"}), None).await;
    assert_eq!(other_tenant.as_array().map(Vec::len), Some(1), "B's project only: {other_tenant}");
    let reply = call(e, "SearchProjects", Some(&w.alice), json!({"workspace": w.wa, "q": "프로그래밍", "page": 1}), None).await.expect("offset page");
    assert_eq!(reply.page.as_ref().map(|p| p["has_more"].clone()), Some(json!(false)));
    let beyond = fails(e, "SearchProjects", Some(&w.alice), json!({"workspace": w.wa, "q": "프로그래밍", "page": 51}), None).await;
    assert_eq!(beyond.code, "AIP.INPUT.INVALID");

    // the compiler said so too
    let src = std::fs::read_to_string(format!("{}/../../examples/saas/app.aip", env!("CARGO_MANIFEST_DIR"))).expect("source");
    let (core, map) = aip_sema::pipeline::check_source(&src).into_core().expect("clean");
    let warned: Vec<String> = aip_pg::compile(&core, &map).diagnostics.iter().map(|d| d.code.clone()).collect();
    assert!(warned.iter().any(|c| c == "AIP-W603"), "{warned:?}");
}

/// The example compiled with one check removed from the plan or the IR.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Off {
    /// the tenant filter of reads (the query is declared `internal cross tenant`)
    Tenant,
    /// `visible to` of the tasks
    Visibility,
    /// the weights: every field counts the same
    Weights,
    /// the id that breaks ties, in the cursor condition
    CursorTie,
    /// the match condition itself: every row is a hit
    Match,
}

async fn engine_without(off: Off, db: &str) -> Arc<Engine> {
    let path = format!("{}/../../examples/saas/app.aip", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(path).expect("example source");
    let (mut core, map) = aip_sema::pipeline::check_source(&src).into_core().expect("saas checks clean");
    match off {
        Off::Tenant => {
            if let Some(aip_ir::Intent::Query(q)) = core.intents.get_mut("SearchTasks") {
                q.cross_tenant = true;
            }
        }
        Off::Visibility => {
            if let Some(t) = core.entities.get_mut("Task") {
                t.visibility = None;
            }
        }
        Off::Weights => {
            for f in core.forms.iter_mut() {
                if let aip_ir::Form::Search(s) = f {
                    s.fields.iter_mut().for_each(|(_, w)| *w = None);
                }
            }
        }
        _ => {}
    }
    let mut program = aip_pg::compile(&core, &map).program;
    if matches!(off, Off::CursorTie | Off::Match)
        && let Some(aip_plan::Intent::Query(q)) = program.intents.get_mut("SearchTasks")
    {
        {
            for v in &mut q.variants {
                let text = &mut v.main.text;
                match off {
                    Off::CursorTie => {
                        let start = text.find("IS NOT DISTINCT FROM (((($").expect("cursor equality");
                        let end = start + text[start..].find("::numeric)").expect("end") + "::numeric)".len();
                        text.replace_range(start..end, "IS NULL");
                    }
                    _ => {
                        let pat = " @@ ";
                        let at = text.find(pat).expect("match condition");
                        let end = at + text[at..].find(".tq").expect("tsquery") + ".tq".len();
                        text.replace_range(at..end, " IS NOT NULL");
                    }
                }
            }
        }
    }
    common::setup_program(db, program).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_negative_controls() {
    // the tenant filter off: someone in both workspaces finds B's row while asking about A
    let w = world(engine_without(Off::Tenant, "aip_e2e_search_no_tenant").await).await;
    let p = probes(&w).await;
    assert!(p.carol_in_a.iter().any(|t| t == "invoice migration of B"), "without the tenant filter the result spans workspaces: {:?}", p.carol_in_a);

    // visibility off: a stranger to the workspace reads its tasks
    let w = world(engine_without(Off::Visibility, "aip_e2e_search_no_visibility").await).await;
    let p = probes(&w).await;
    assert!(!p.stranger.is_empty(), "without 'visible to' a stranger finds rows: {:?}", p.stranger);
    assert!(p.carol_in_a.len() == 3, "the tenant filter still holds: {:?}", p.carol_in_a);

    // weights off: a title hit and a note hit score the same
    let w = world(engine_without(Off::Weights, "aip_e2e_search_no_weights").await).await;
    let p = probes(&w).await;
    let ranks: Vec<f64> = p.weighted.iter().map(|(_, r)| *r).collect();
    let all = search(&w.e, &w.alice, &w.wa, "migration").await;
    assert!(all.iter().all(|r| (rank(r) - rank(&all[0])).abs() < 1e-9), "without weights the scores are equal: {ranks:?}");

    // ties no longer broken by the id in the cursor condition: a page boundary inside equal scores loses rows
    let w = world(engine_without(Off::CursorTie, "aip_e2e_search_no_tie").await).await;
    let p = probes(&w).await;
    let seen: BTreeSet<&str> = p.paged.iter().map(String::as_str).collect();
    assert!(p.paged_expected.iter().any(|i| !seen.contains(i.as_str())), "without the tie-break rows are skipped between pages");

    // no match condition: the word is not needed to be a hit
    let w = world(engine_without(Off::Match, "aip_e2e_search_no_match").await).await;
    task(&w.e, &w.alice, &w.pa, "invoice", None).await;
    task(&w.e, &w.alice, &w.pa, "something else", None).await;
    let rows = search(&w.e, &w.alice, &w.wa, "invoice").await;
    assert!(rows.iter().any(|r| r["title"] == json!("something else")), "without the match condition every row is a hit: {rows:?}");
}
