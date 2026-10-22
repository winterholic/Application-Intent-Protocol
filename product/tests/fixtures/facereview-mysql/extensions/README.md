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

- The Redis pool cache is read here and written by the job of WBS 7.2.7 (`slice14a_pool.py`, "Slice14-A" below). A cold, expired or
  unreachable cache is built inside the request, as the original does.
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
- `POST home/video/recommend` (7.2.9): ported, see "Slice14-G" below.

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

## Slice14-A: the offline recommendation pool refresh (WBS 7.2.7)

`slice14a_pool.py` ports the Celery beat task `rebuild_recommendation_pool` (`new-facereview@7948b7e7`, `common/tasks/scheduled_tasks.py`,
every 30 minutes) as the durable job `rebuildRecommendationPool`, declared in `facereview.aip` with
`access read.Video, mongo.video_distribution, redis "facereview:reco:*"`. A server schedule runs it as a service principal bound to an
administrator (`schedules: [{name, operation: "rebuildRecommendationPool", cron: "*/30 * * * *", principal}]`, see `crates/aip-cli/tests/mysql_facereview_pool_job.rs`).

| Original | AIP |
|---|---|
| `SET facereview:reco:pool <json> EX 7200` | `ctx.redis.set_ex(..., 7200, effect_key="pool")` |
| `DEL`, `HSET` per category, `EXPIRE 7200` of `facereview:reco:category` | `ctx.redis.replace_hash_ex(...)` (one Lua script on the Redis side) |
| (the hash comes back in Redis's internal order) | `facereview:reco:category_order`: the order of the original's own category list, so a warm cache answers in the order a cold request does |

`Video.homeCategory` and `Video.homePersonalized` read the keys first and build the pool themselves when they are absent, expired or Redis
does not answer. `reference/slice14a_oracle.py` runs the original `_build_and_cache_ranked_pool` against its own Redis database and the job
against another on the same data: pool (19 entries), category fields (6 categories), category order and expiry all matched.

Differences and limits: a read extension has 64 ctx calls, so a request that has to build the pool by itself fails loudly beyond about
3000 active videos (the original has no bound); the job needs `limits.job_calls` (about `MAX_VIDEOS / 50`, 400 for the 15000 it reads) and
fails with `WIRE.COST_EXCEEDED` otherwise, leaving the previous cache as it was. The three writes are not one transaction (the original's
Redis pipeline is not atomic across a crash either). The Video read budget gained `cursor`: paging by `offset` costs more with every page
(`limit x (1 + aggregates) + offset` against `cost 1000`), so a read of two aggregates could not go past about 700 rows deep.

## Slice14-B: the daily trending collection (WBS 7.2.7)

`slice14b_trending.py` ports `YoutubeTrendingJob` (`new-facereview@7948b7e7`, `common/scheduler/jobs/youtube_trending_job.py`, daily 06:00 Asia/Seoul)
as the job `fetchTrendingVideos`, with `youtube_common.py` (duration, saving a video) shared with the category fill of Slice14-C. The job
declares `access read.Video, create.Video, http.youtube_videos, mongo.video_distribution, mongo.video_timeline_emotion_count`; the service
configuration holds the query-string integration (`method` GET, `query_params` part/chart/regionCode/maxResults/pageToken,
`secret_query_env` for the API key) and `workers.mongo.write_collections` for the two collections. `Video` gained an administrator-only
`expose create` (the original's `_save_video` and, later, the approval of a video request) and the filter `youtube_url.in`.

| Original | AIP |
|---|---|
| `requests.get(.../youtube/v3/videos, params)` with the key in the URL, 4 pages of 50 | `ctx.http.call("youtube_videos", {...}, key="trending:page:N")`; the key is the server's |
| `Video(...)`, `db.session.add`, `flush` | `ctx.data.create("Video", {...}, key="video:<youtube id>")` |
| `VideoDistributionRepository.upsert`, `VideoTimelineEmotionCountRepository.upsert` | `ctx.mongo.update_one(..., upsert=True)` with `$set` and `$setOnInsert`, which the server sends again after a lost answer |
| `existing = Video.query.filter(youtube_url.in_(ids))` | `ctx.data.read` with `youtube_url.in` in chunks of 50, then the unique `youtube_url` as the backstop (`WIRE.ALREADY_EXISTS` is skipped) |

`reference/slice14b_oracle.py` runs the original class on the same data: its category rules on 33 cases and the duration rules on 15
(all equal), then its `execute()` against a stand-in YouTube and the AIP job against another: 6 new rows and 12 MongoDB documents equal.
Two intended differences: a video without a title (the original saves an empty title) is skipped, and an empty channel name is NULL.

Differences and limits: a call that cannot be made (the provider does not answer) asks for the run to be tried again after 5 seconds
instead of ending the pages (a non-2xx answer still ends them, as in the original); each new video costs 3 ctx calls (row, two
documents), so a run of 200 new videos needs `limits.job_calls` of about 700. The original saves a video even if the second MongoDB write
fails (it logs and goes on); here the job is tried again and the journal replays the row.

## Slice14-C: the category fill (WBS 7.2.7)

`slice14c_category_fill.py` ports `YoutubeCategoryFillJob` (`new-facereview@7948b7e7`, `common/scheduler/jobs/youtube_category_fill_job.py`,
Tuesday and Friday 03:00 Asia/Seoul) as the job `fillCategoryVideos`. It declares `access read.Video, create.Video, http.youtube_search,
http.youtube_videos, mongo.video_distribution, mongo.video_timeline_emotion_count`. The service configuration adds a second query-string
integration `youtube_search` (`query_params` part/q/type/regionCode/relevanceLanguage/maxResults/order) and gives `youtube_videos` the
`id` parameter; both use `secret_query_env` for the API key. The plan is the original's: count the active videos of each genre, take the
genres under 20 (the five lowest when none is), 1 to 3 queries each by the shortage, at most 12 searches, a search costs 100 and a details
call 1, and a 403 sets the used quota past the limit.

Two details of the original that the port keeps: the quota is checked only between genres, so after a 403 the rest of that genre's
queries are still searched; and a video found under two genres is saved under the first (the second create meets the unique
`youtube_url`). The create key is `video:<youtube id>:<genre>`, because the same id searched under another genre is another request.
The original counted genres with `GROUP BY`; the `Video` read has no grouping, so the job pages the active videos (100 per ctx call) and
counts them.

`reference/slice14b_oracle.py` also runs the original `execute()` against the same stand-in (two ids the database has and three new ones per
query, plus a channel result that is not a video): 36 new rows in 4 genres, 72 MongoDB documents and the quota (1212 in 12 searches) equal.
A run saves up to 36 videos with 3 ctx calls each plus about 70 reads and calls, so the service sets `limits.job_calls` to 1000.

## Slice14-D: the approval of a video request (WBS 7.2.9)

`slice14d_approve.py` ports `AdminService.approve_video_request` (`new-facereview@7948b7e7`, `app/services/admin_service.py`) as the job
`approveVideoRequest(request, youtube_title, channel_name, duration, category)`, for administrators. It declares `access read.VideoRequest,
read.Video, create.Video, VideoRequest.accept, mongo.video_distribution, mongo.video_timeline_emotion_count`, and reuses `save_video` of
`youtube_common.py` (the Video row through the exposed create and the two initial documents as upserts). `VideoRequest` gained the transition
`accept` (administrator, from `PENDING`, and only while a `Video` with the request's `youtube_url` exists, so a client cannot mark a request
accepted without registering the video; the `reject` that was already there needs no video).

| Original | AIP |
|---|---|
| request must exist and be `PENDING`, else `VIDEO_NOT_FOUND` / `INVALID_INPUT_VALUE` | the job reads the request and fails with that text in the error |
| no `Video` with the request's `youtube_url`, else `VIDEO_DUPLICATE_URL` | the unique `youtube_url` refuses the create (`WIRE.ALREADY_EXISTS`), the job fails |
| `Video(...)`, two repository upserts, `status = 'ACCEPTED'` in one union transaction with a saga | create, two upserts and the transition `accept`, each journaled; a retry replays what was done and finishes the rest |
| the response carries the new `video_id` | the job result carries it (`/jobs/result`) |

`reference/slice14b_oracle.py` runs the original method and the job on the same data: the video row (`reqvideo001`, title, channel, genre,
length, 0 views, active), the two documents and the request status (`ACCEPTED`, no comment) are equal. Differences: the approval runs in the
background (the administrator reads the result), a job has no saga (a failure between the create and the acceptance is repaired by the
retry, not undone), and a request approved while a soft-deleted video holds the url fails like a duplicate.

## Slice14-E: the my page emotion screens (WBS 7.2.8)

`slice14e_mypage.py` ports `MypageService.get_recent_videos`, `get_highlight`, `get_emotion_calendar`, `get_moments` and `get_emotion_dna`
(`new-facereview@7948b7e7`, `app/services/mypage_service.py`) as the read extensions `User.myRecentVideos`, `myHighlight`,
`myEmotionCalendar`, `myMoments` and `myEmotionDna`. Each answers `{body: Text}`, the original's JSON document as text. They declare
`access mongo.user_watch_stats`, a derived collection (decision D-P-USERSTATS, like `video_emotion_timeline`): the server reads only the
verified caller's `youtube_watching_data` sessions and answers the commands `sessions` (latest sessions without timelines), `compress`
(the 40 bucket sums of chosen sessions), `highlight` (latest 500 shares), `calendar` (rows of a period with the length of the timeline),
`moments` (peak frames of the latest 200 sessions, one per 60 frame window) and `dna` (totals). No session document or per-frame value
reaches the extension. The rules of the screens (rounding with Python `round`, grouping, the DNA types, traits and fun facts) are the
original's code, copied.

`reference/slice14e_oracle.py` runs the original service and the extensions on 15 generated sessions (dictionary and array timelines,
130 frame sessions, an unparsable key, an empty timeline, sessions of a deleted video and of a missing video, midnight and
millisecond edges): 66 cases (recent 18, highlight 3, calendar 21, moments 21, DNA 3) equal, and `slice14e-golden.json` plus
`slice14e-sessions.json` record them for the CI replay in `mysql_facereview_trending_job`. Mutations of the window size and of the peak
threshold fail the oracle.

Differences and limits: soft-deleted videos are invisible (the original's moments and DNA also count them); the DNA is computed on every
call (the original caches it in `user_emotion_dna` for 7 days); the DNA reads the genres of at most about 3000 distinct videos; an optional
input such as `month` must be sent as null; `GET mypage/emotion/summary` is Slice14-F below.

## Slice14-F: the emotion summary (WBS 7.2.8)

`slice14e_mypage.py` also ports `MypageService.get_emotion_summary` as the write extension `User.myEmotionSummary` (`POST /apply` with
`{extension, input: {}}` and a fresh `key` per request). It reads the member's `UserEmotionSummary` row (`ctx.data.read`, own row only),
asks the server command `emotion_seconds` of `user_watch_stats` for the totals of the sessions the checkpoint has not seen, and writes the
new totals with the transition `UserEmotionSummary.applyDelta` (`patch` the five totals and the checkpoint, `expect lock_version`,
`to lock_version = this.lock_version + 1`). A member without a row gets one first (`ctx.data.create`; the zero totals come from the column
defaults of `db-defaults.sql`).

| Original | AIP |
|---|---|
| `UserEmotionSummary(user_id=...)` on the first call | `ctx.data.create` of the exposed create (`allow id = actor`) |
| no checkpoint: every session once, `most_emotion_timeline` per second (the last frame of a second wins), else `emotion_percentages` times the estimated watch seconds; checkpoint `now` and `''` | server command `emotion_seconds` without `after`, the same rules; the sums stay unrounded and the extension truncates the total like `int()` |
| with a checkpoint: `finalized_at > t or (finalized_at = t and video_view_log_id > id)`, ordered by both, the stored `emotion_seconds`, else the estimate (the timelines are not read) | `emotion_seconds` with `after: {at, session}`, the same query and order |
| `apply_delta(expected_version, ...)` updates `WHERE lock_version = expected`, three attempts | the patch with `expect: {lock_version}`; a lost race is `WIRE.CONFLICT` and nothing changes |
| no new session: no write | no write, `lock_version` unchanged |

`reference/slice14f_oracle.py` runs the original and the port from the same starting row and compares the answer and the row afterwards
(totals, checkpoint, session id, `lock_version`): 19 cases (no row, a row without a checkpoint, ties on `finalized_at`, a checkpoint before,
inside, after and at the end of the history, stored seconds, timelines, percentages, `timeline_len`, `frame_count`, text and out-of-range
numbers, fractions of a second, 800 sessions, a member with no session, a repeated call, a late session added exactly once, concurrent
calls, a guest). `slice14f-golden.json` and `slice14f-sessions.json` record them for the CI replay in `mysql_facereview_trending_job`.

Differences and limits: (1) **The checkpoint column.** The original keeps `last_finalized_at` in a `DATETIME` without fractions, which
rounds the checkpoint of a session finalized at `.4 s` down, so the next call counts that session again (measured on the original: 40, 50
and 60 seconds on three calls with one new session). `db-defaults.sql` makes the column `datetime(3)`, as precise as MongoDB's
`finalized_at`; the original keeps working on it and counts once. (2) A first call that races another first call fails
(`WIRE.ALREADY_EXISTS` from the create) instead of reusing the row, and a call that loses the `lock_version` is `WIRE.CONFLICT` at once
where the original tried three times: send the request again. (3) The transition is public (`expose apply`), so a member can write their
own totals directly. Nothing else reads the table (checked in the original's code), so only their own summary screen is affected; a
transition that only extensions may call would close it. (4) A session needs `finalized_at` and `video_view_log_id` for the later calls,
or the call fails. (5) One call sums at most 20000 sessions and fails beyond that instead of answering with a partial total.

## Slice14-G: video request batch (WBS 7.2.9)

`slice14g_request.py` ports `POST home/video/recommend` (`new-facereview@7948b7e7`, `home_service.py` `create_user_video_recommend`) as the
write extension `VideoRequest.recommend`. It creates one `VideoRequest` per URL through the exposed create (access `create.VideoRequest`)
in one transaction; the full YouTube URL is the database default of `youtube_full_url` and the unique `youtube_url` (binary collation)
refuses a URL anyone requested before, also for two requests at the same moment. Decision D-P-RECOMMEND, evidence in `product/VERIFICATION.md`
and `reference/slice14g_oracle.py` (golden `slice14g-golden.json`).

| Original | AIP | Notes |
|---|---|---|
| `POST home/video/recommend` | `VideoRequest.recommend` | input `user` (the caller's own id) and `youtube_url_list` (JSON array text, 1 to 20 ids); output `requested` |
| `existing_urls.intersection(...)` then `bulk_save_objects` | one exposed create per id, unique `youtube_url` | any refused id cancels the whole list; the duplicate answers `WIRE.ALREADY_EXISTS` (the original: 409 V004) |
| `youtube_full_url=f"https://www.youtube.com/watch?v={url}"` | database default (`db-defaults.sql`) | the caller cannot write it |

Differences: the same id twice in one list is refused (the original creates two rows); more than 20 ids and an id over 50 characters are refused;
an invalid list answers `WIRE.EXTENSION_ERROR`. Before `db-defaults.sql` adds the unique key, remove duplicate `youtube_url` rows
(the query is in that file).

## Slice14-H: administrator dashboard (WBS 7.2.9)

`slice14h_dashboard.py` holds the two dashboard parts an aggregate cannot give (`new-facereview@7948b7e7`, `admin_service.py`
`_get_video_request_pipeline` and `_get_content_health`); everything else is an administrator-only aggregate the client calls.
Decision D-P-DASHBOARD, evidence in `product/VERIFICATION.md` and `reference/slice14h_oracle.py` (golden `slice14h-health.json`).

| Original | AIP | Notes |
|---|---|---|
| `get_signup_trend(period)` | `User.signupCount(from, to)` per bucket | the client makes the buckets (7d/30d day, 3m week, 1y/3y month) and calls once per bucket |
| `_get_weekly_active_users` | `User.weeklyActive(since)` | members with a view log at or after `since`; no Redis cache |
| `_get_video_request_pipeline` | `VideoRequest.pendingCount` and extension `VideoRequest.pipeline` | the average reads the processed rows of the last 30 days (50 pages of 100) |
| `_get_content_health`, Mongo | extension `Video.contentHealth` over `mongo.video_distribution_stats` | the server runs the original `$facet` pipeline |
| `_get_content_health`, `category_top5` | `Video.categoryViews(category)` per genre | genres without a visible video are absent: check `Video.totalCount` with `category.eq` first |

Both extensions call an administrator-only aggregate first, so anyone else ends with `WIRE.EXTENSION_ERROR` (a guest with `WIRE.UNAUTHENTICATED`).
