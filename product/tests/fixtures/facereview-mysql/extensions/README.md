# Slice13-B: FaceReview home list extensions (WBS 7.2.3)

Since WBS 7.2.3 the canonical `facereview.aip` declares these extensions itself (with `access mongo.<collection>`), together with
the `id.in`/`video.in` filters, the two enums and the public `likeCount`/`commentCount` aggregates of `Video`. `aip service serve`
therefore serves them directly when `workers.directory` holds the Python modules and `workers.mongo` names the MongoDB.

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

1. Resolved by WBS 7.2.3: the definition grammar now declares `access mongo.<collection>` (`spikes/spike-v1-fixture/tests/mongo_access.rs`),
   so the canonical definition holds the grants and the in-process host of `slice13b_host.rs` is no longer the only way to serve it.
2. Extension input and output accept scalar types only (`Json` and lists are refused by the value boundary).
3. `/extension` requires a signed session (`WIRE.UNAUTHENTICATED`), so the original's `login_optional` guests of `home/search`
   and `home/video/all` cannot be served.
4. The host's Mongo `sort` object loses key order (the JSON map is alphabetical), so `[('created_at', -1), ('_id', -1)]` cannot be
   expressed to the host. The extension rebuilds the order itself: id-list paths sort all matching documents by (`created_at`, `_id`)
   client-side; `video/all` pages a window around the slice with `sort_by created_at` and re-sorts equal-`created_at` groups by `_id`.
   More than 100 documents with one identical `created_at` next to a page, or an unstable host answer, fails loudly. A host-side
   ordered multi-key sort would remove the window reads. `S13B_TIE_PROBE=1` seeds a tie group that straddles pages (passes).
5. Extension error messages are replaced by a fixed text, so a failing extension shows only `WIRE.EXTENSION_ERROR`.

## Slice13-D: recommendation lists (WBS 7.2.3)

`slice13d_recommend.py` ports three more original endpoints (`new-facereview@7948b7e7`, `home_service.py`, `watch_service.py`).
The arithmetic is `recommendation_math.py` and `recommendation_pool_builder.py` (pinned against the original by
`product/tests/slice13d_*_oracle_test.py`); the extension gathers the inputs the original reads.

| Original | Extension | Original method | Output |
|---|---|---|---|
| `GET home/category` | `Video.homeCategory` | `get_videos_by_category_emotions` | `categories`: JSON list of `{category_name, videos}` |
| `GET home/personalized` | `Video.homePersonalized` | `get_personalized_videos` (limit 20) | `videos`: JSON list |
| `GET watch/recommended` | `Video.watchRecommended` | `get_recommended_videos` | `{videos, total, page, size, has_next}` as in 13-B |

Verification: `reference/slice13d_oracle.py` seeds a run-owned MySQL 8.4 and MongoDB from `slice13d-seed.sql` and the two JSON files
(written by `reference/slice13d_make_seed.py`), serves the canonical definition with the real `aip service serve` (`workers.mongo`,
Python worker sandbox, JWKS-verified bearer tokens) and compares 41 answers with the original's own `HomeService`/`WatchService`
on the same data: 4 callers for the category lists, 5 for the personalized list (including a member who has watched everything),
and 14 related-video situations for two callers (shared and tied dominant emotion, null dominant emotion, no distribution
document, pagination past the end, unknown/soft-deleted/NULL-flag video). All 41 matched; three consecutive runs wrote an identical
`slice13d-golden.json`. `crates/aip-cli/tests/mysql_facereview_recommend.rs` replays the golden answers on a fresh deployment in CI.

The seed is arranged so that the original does not depend on time or chance: every scored video is older than the 55 days after which
the freshness term sits at its floor, each category has at most one video that can only be padding (so `random.shuffle` of the
padding source has one outcome), and the pool is smaller than 150 (no exploration sample).

### Differences from the original (deliberate, or limits found)

- No Redis pool cache: the original's request path builds the pool when the cache is cold, and so does this port, every time. A read
  extension cannot write the cache. The offline refresh job belongs to WBS 7.2.7.
- At most 5000 active videos (and 5000 view-log rows of the caller) are gathered; beyond that the call fails loudly. The original
  is unbounded.
- Random draws (padding of thin categories, the exploration sample of the personalized list, the fallback sample) are the
  original's own `random` calls and are not reproducible.
- The favourite genres order: `user.favorite_genres.all()` is served by MySQL from the unique index `(user_id, genre)`, i.e. in ENUM
  declaration order, so the original's favourite "rank" is the position in `GenreEnum`, not the order the member picked them. The
  port reproduces that (the first run of the oracle found it: the picked order gave a different list for Alice).
- `get_personalized_videos` raised USER_NOT_FOUND for a missing user; an inactive or unknown caller is refused by the row
  policies instead and gets an empty list.
- The filter grammar has no `!=`: the current video is dropped from the same-category list after the read.
- `watch/recommended` of an unknown, soft-deleted or NULL-flag video is `WIRE.EXTENSION_ERROR` (original: 404 VIDEO_NOT_FOUND).
- The host's `youtube_watching_data` access is private to the caller and the projection allowlist now includes `emotion_percentages`
  (`spikes/spike-v4-worker/src/capability.rs`): a negative projection cannot drop the multi-megabyte `emotion_score_timeline`, and a
  full document would exceed the 1 MB result cap.

### Not ported in WBS 7.2.3 (handed to the named items)

- `GET watch/video` (detail): ported by WBS 7.2.5, see "Slice13-F" below.
- `POST home/video/recommend` (7.2.9): `VideoRequest.create` exists, but the original derives `youtube_full_url` on the server, rejects
  a URL that any member requested before (the caller cannot read other members' requests) and stores the batch in one transaction.
  The derived value and the global duplicate check need a computed default and a unique index on `video_request.youtube_url`.

## Slice13-F: watch detail (WBS 7.2.5)

`slice13f_watch.py` ports `GET watch/video` (`new-facereview@7948b7e7`, `watch_service.py` `get_video_detail`):

| Original | AIP | Notes |
|---|---|---|
| `GET watch/video` | `Video.watchDetail` (read extension) | the original's `VideoDetailDto`, `timeline_data` as JSON text |
| `video.view_count += 1` inside the GET | `Video.view` (transition, `apply`) | the caller applies it after the read; the read answers with the count from before |

The all-viewer timeline cannot come from `mongo.youtube_watching_data` (private to the caller). The extension declares
`access mongo.video_emotion_timeline`, a **derived** collection of the Mongo capability (`spikes/spike-v4-worker/src/capability.rs`
`video_timeline`): the server reads the video's sessions, adds the scores key by key in the original's order and returns only the 100
bucket averages. No session document, user id or per-session value leaves, and the grant does not open the raw collection.

### Differences from the original (deliberate, or limits found)

- `view_count` is `NOT NULL` after `db-defaults.sql` (backfill of stray NULLs, `DEFAULT 0`): the definition language counts only on a
  non-null Int, and the original never writes NULL (it returns a 500 on one).
- A video with no usable timeline key gets the original's default timeline (one point `x=1, y=0.0` per emotion). A session whose
  `emotion_score_timeline` is not an object, an array shorter than 5 and a key that is not a number are skipped, as the original does
  (a non-object timeline crashes the original with a 500). A score that is not a number fails loudly.
- At most 50000 sessions and 5000000 timeline entries of one video are read; beyond that the call fails loudly. The original is unbounded.
- Keys are summed in the original's order (sessions in natural order, a session's keys in stored order) with plain float addition. Mongo's
  own `$sum` compensates its rounding error, which moved one bucket sitting exactly on a rounding boundary (48.1 against 48.2) in the first run.
- A negative or non-finite key is skipped (the original would wrap around or crash).
- `GET watch/video` of an unknown or soft-deleted video is `WIRE.EXTENSION_ERROR` (original: 404 VIDEO_NOT_FOUND). Guests are refused by
  the extension route (see the 7.2.3 list above).

`reference/slice13f_oracle.py` runs the original `get_video_detail` (which commits its own increment; the oracle records `+1` and restores
the row) against the same MySQL 8.4 and MongoDB as the real server for 3 callers x 12 videos, then applies `Video.view`.
`crates/aip-cli/tests/mysql_facereview_watch.rs` replays `slice13f-golden.json`. `reference/slice13f_make_seed.py` writes the sessions.
