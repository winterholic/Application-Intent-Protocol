# Slice13-B: FaceReview home list extensions (WBS 7.2.3)

The pure Slice13-D recommendation math and ranked-pool helpers live in
`recommendation_math.py` and `recommendation_pool_builder.py`. They are not
bound to an AIP definition or service route yet; the accompanying original-source
oracle tests run separately with `FACEREVIEW_SOURCE_PATH` set.

`slice13b_lists.py` ports three original endpoints (`new-facereview@7948b7e7`, `app/services/home_service.py`):

| Original | Extension | Original method |
|---|---|---|
| `GET home/search` | `Video.homeSearch` | `get_search_videos` |
| `GET home/video/all` | `Video.homeVideoAll` | `get_all_videos` |
| `GET home/bookmark` | `Video.homeBookmark` | `get_bookmark_videos` |

Output is `{videos, total, page, size, has_next}`. `videos` is a **Text** field holding the JSON array of the original
`BaseVideoDataDto.to_dict()` objects, because read extensions accept only scalar output types. `homeSearch` takes `emotions` as a
comma separated Text for the same reason.

## What is verified and how

`reference/slice13b_oracle.py` seeds a run-owned MySQL 8.4 and MongoDB, adopts the definition with the real `aip` binary, serves it
with `crates/aip-service/tests/slice13b_host.rs` (real MySQL adapter, real MongoDB driver capability, Python worker process) and
compares each HTTP answer with the **original `HomeService` imported from the pinned checkout** running against the same databases.
Environment variables are listed in the script's docstring; it has no default endpoint or credential.

## Independent reviewer oracle

`reference/slice13b_review_oracle.py` runs the 27 B cases of the reviewer's frozen `oracle_cases.json` (pinned original on SQLite and
mongomock) against the same seed loaded into real MySQL 8.4 and mongod, and also runs the original HomeService on that MySQL
(authoritative for the collation cases B-s-4 and B-s-8). Intended differences (deleted videos hidden by row policy, guests refused by
the host, a stand-in user for B-bm-4) are printed per case. Guest cases additionally check the data logic through a member without
bookmarks.

## Differences from the original, all deliberate

- Deleted videos are not shown. The original looks videos up with `video_id IN (...)` without `is_deleted`, so `video/all` and
  `bookmark` can show them; AIP's `Video` row policy hides them and the policy is not relaxed. `total` still counts the Mongo
  documents, as the original does, so `total` and the list length can differ.
- The order of the original's RDB `IN` lookup is unspecified. This port answers in Mongo page order.
- Where the original is unbounded (emotion-filtered search, bookmark list) the extension chunks within the host limits (RDB `in` 50,
  Mongo `$in` 100) up to 5000 candidate videos and then fails loudly.
- The search `total` without an emotion filter is counted by paging ids because an extension cannot pass a caller filter to the
  `totalCount` aggregate.

## Core and grammar dependencies found (not changed here)

1. The definition grammar cannot declare `access mongo.<collection>` (`aip service check` answers `DEF.UNKNOWN_SYMBOL`). The host
   test adds the grant to the compiled facts, as the Slice13-A bridge tests do. A definition served by `aip service serve` cannot
   hold it today.
2. Extension input and output accept scalar types only (`Json` and lists are refused by the value boundary).
3. `/extension` requires a signed session (`WIRE.UNAUTHENTICATED`), so the original's `login_optional` guests of `home/search`
   and `home/video/all` cannot be served.
4. The host's Mongo `sort` object loses key order (the JSON map is alphabetical), so `[('created_at', -1), ('_id', -1)]` cannot be
   expressed to the host. The extension rebuilds the order itself: id-list paths sort all matching documents by (`created_at`, `_id`)
   client-side; `video/all` pages a window around the slice with `sort_by created_at` and re-sorts equal-`created_at` groups by `_id`.
   More than 100 documents with one identical `created_at` next to a page, or an unstable host answer, fails loudly. A host-side
   ordered multi-key sort would remove the window reads. `S13B_TIE_PROBE=1` seeds a tie group that straddles pages (passes).
5. Extension error messages are replaced by a fixed text, so a failing extension shows only `WIRE.EXTENSION_ERROR`.
6. The canonical `facereview.aip` is not edited. The driver derives the definition it adopts from it by adding `id.in` to
   `Video`'s filter list, `video.in` to `VideoBookmark`'s filter list, two enums and the three extension declarations (without the
   Mongo access line).
