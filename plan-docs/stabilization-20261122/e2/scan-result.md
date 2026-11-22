# 확장 access 선언과 코드의 비교 (스캔 결과 전체)

- 생성: `python3 plan-docs/stabilization-20261122/e2/scan_extensions.py` (정의는 `/tmp/aip-stab-e-target/debug/spike-v1-fixture facts`로 읽음), 기준 커밋 `45e7b57c`, 2026-11-22.
- 텍스트 스캔이라 증거일 뿐 검증기가 아니다. 계산된 이름의 호출은 `computed name`으로, 함수 밖(공용 모듈) 호출은 이 확장에 귀속되지 않는다. `not found`는 구현 파일이나 함수를 이 저장소에서 못 찾은 것이다(시험이 파일을 생성하는 경우 등).

| definition | extension (kind) | declared access | observed in code | gaps |
|---|---|---|---|---|
| plan-docs/expressiveness-1.0/fixtures/run_app.aip | extensions.completeAndRepeat (write) | Todo.complete, create.Todo, read.Todo | - | function not found |
| plan-docs/expressiveness-1.0/fixtures/s11_pg_ext_create.aip | extensions.completeAndRepeat (write) | Todo.complete, create.Todo, read.Todo | - | function not found |
| plan-docs/expressiveness-1.0/fixtures/s14_job_notify.aip | operations.renderPdf (job) | - | - | implementation source not found |
| porting/flaskbb/app.aip | operations.sendActivationEmail (job) | http.mail | - | none |
| porting/flaskbb/app.aip | operations.sendPasswordResetEmail (job) | http.mail | - | none |
| porting/flaskbb/app.aip | extensions.moveToForum (write) | Forum.authorizeTopicMove, Forum.refreshTopicMoveProjection, Topic.relocate, read.Topic | read: Topic; apply: Forum.authorizeTopicMove, Forum.refreshTopicMoveProjection, Topic.relocate | none |
| porting/flaskbb/tests/fixtures/topic-move-internal-grant-repro.aip | extensions.moveToForum (write) | Topic.relocate | read: Topic; apply: Forum.authorizeTopicMove, Forum.refreshTopicMoveProjection, Topic.relocate | undeclared read Topic; undeclared apply Forum.authorizeTopicMove; undeclared apply Forum.refreshTopicMoveProjection |
| porting/infisical/app.aip | operations.secretReminder (job) | apply.Secret.remind, read.Secret | - | function not found |
| porting/infisical/tests/fixtures/encrypted-app.aip | operations.secretReminder (job) | apply.Secret.remind, read.Secret | - | function not found |
| product/tests/fixtures/expr-apps/orders/orders.current.aip | extensions.place (write) | Order.finish, Product.setStock, read.OrderLine, read.Product | read: OrderLine, Product; apply: Order.finish, Product.setStock | none |
| product/tests/fixtures/expr-apps/orders/orders.target-b.aip | extensions.placeAtomic (write) | Order.settle, Product.take, create.Order, create.OrderLine, read.Product | read: Product; apply: Order.settle, Product.take; create: Order, OrderLine | none |
| product/tests/fixtures/external-effects/effects.aip | extensions.track (write) | Video.view, mongo.youtube_watching_data, redis.effects:counter:requests, redis.effects:dedupe:* | apply: Video.view; mongo: youtube_watching_data; redis: effects:counter:requests | declared redis effects:dedupe:* not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | operations.approveVideoRequest (job) | apply.VideoRequest.accept, create.Video, mongo.video_distribution, mongo.video_timeline_emotion_count, read.Video, read.VideoRequest | read: VideoRequest; apply: VideoRequest.accept | declared read Video not seen; declared create Video not seen; declared mongo video_distribution not seen; declared mongo video_timeline_emotion_count not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | operations.fetchTrendingVideos (job) | create.Video, http.youtube_videos, mongo.video_distribution, mongo.video_timeline_emotion_count, read.Video | - | declared read Video not seen; declared create Video not seen; declared mongo video_distribution not seen; declared mongo video_timeline_emotion_count not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | operations.fillCategoryVideos (job) | create.Video, http.youtube_search, http.youtube_videos, mongo.video_distribution, mongo.video_timeline_emotion_count, read.Video | - | declared read Video not seen; declared create Video not seen; declared mongo video_distribution not seen; declared mongo video_timeline_emotion_count not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | operations.generateDummyData (job) | apply.Video.view, create.VideoViewLog, mongo.video_distribution, mongo.video_session_stats, mongo.youtube_watching_data, read.Video | apply: Video.view; create: VideoViewLog; mongo: video_distribution, youtube_watching_data | declared read Video not seen; declared mongo video_session_stats not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | operations.rebuildRecommendationPool (job) | mongo.video_distribution, read.Video, redis.facereview:reco:* | - | declared read Video not seen; declared mongo video_distribution not seen; declared redis facereview:reco:* not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.myEmotionCalendar (read) | mongo.user_watch_stats | - | declared mongo user_watch_stats not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.myEmotionDna (read) | mongo.user_watch_stats | - | declared mongo user_watch_stats not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.myEmotionSummary (write) | UserEmotionSummary.applyDelta, create.UserEmotionSummary, mongo.user_watch_stats, read.UserEmotionSummary | read: UserEmotionSummary; apply: UserEmotionSummary.applyDelta; create: UserEmotionSummary | declared mongo user_watch_stats not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.myHighlight (read) | mongo.user_watch_stats | - | declared mongo user_watch_stats not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.myMoments (read) | mongo.user_watch_stats | - | declared mongo user_watch_stats not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.myRecentVideos (read) | mongo.user_watch_stats | - | declared mongo user_watch_stats not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.recordFrame (write) | mongo.youtube_watching_data, read.Video, read.VideoViewLog, redis.facereview:watch:frame:* | read: Video, VideoViewLog; mongo: youtube_watching_data | declared redis facereview:watch:frame:* not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.startWatching (write) | create.VideoViewLog, read.VideoViewLog, redis.facereview:watch:active:* | read: VideoViewLog; create: VideoViewLog | declared redis facereview:watch:active:* not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.systemStatus (read) | User.totalCount, mongo.server_status, redis.facereview:metrics:* | aggregate: User.totalCount | declared mongo server_status not seen; declared redis facereview:metrics:* not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.temporalContext (read) | User.totalCount | aggregate: User.totalCount | none |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.contentHealth (read) | User.totalCount, mongo.video_distribution_stats | aggregate: User.totalCount | declared mongo video_distribution_stats not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.homeBookmark (read) | mongo.video_distribution | - | declared mongo video_distribution not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.homeCategory (read) | mongo.video_distribution, redis.facereview:reco:* | - | declared mongo video_distribution not seen; declared redis facereview:reco:* not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.homePersonalized (read) | mongo.video_distribution, mongo.youtube_watching_data, redis.facereview:reco:* | - | declared mongo video_distribution not seen; declared mongo youtube_watching_data not seen; declared redis facereview:reco:* not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.homeSearch (read) | mongo.video_distribution | read: Video | undeclared read Video (read extensions have no declaration for reads); declared mongo video_distribution not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.homeVideoAll (read) | mongo.video_distribution | - | declared mongo video_distribution not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.watchDetail (read) | mongo.video_emotion_timeline | read: Video, VideoLike | undeclared read Video (read extensions have no declaration for reads); undeclared read VideoLike (read extensions have no declaration for reads); declared mongo video_emotion_timeline not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.watchRecommended (read) | mongo.video_distribution | read: Video | undeclared read Video (read extensions have no declaration for reads); declared mongo video_distribution not seen |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.pipeline (read) | VideoRequest.pendingCount | read: VideoRequest; aggregate: VideoRequest.pendingCount | undeclared read VideoRequest (read extensions have no declaration for reads) |
| product/tests/fixtures/facereview-mysql/facereview.aip | extensions.recommend (write) | create.VideoRequest | create: VideoRequest | none |
| product/tests/fixtures/mysql-worker-action/video-action.aip | extensions.view (write) | Video.increment | apply: Video.increment | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.eventJob (job) | events.audit | - | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.fileJob (job) | files.docs | - | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.handleJob (job) | - | - | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.hookJob (job) | apply.Video.increment | apply: Video.increment | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.incrementJob (job) | apply.Video.increment | apply: Video.increment | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.mailJob (job) | http.mail | - | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.messageJob (job) | messages.orders | - | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.nestedJob (job) | apply.Video.increment, jobs.incrementJob | - | declared apply Video.increment not seen |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.relayJob (job) | messages.orders | - | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.retryJob (job) | apply.Video.increment | apply: Video.increment | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.slowJob (job) | - | - | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.tickJob (job) | - | - | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | operations.viewedJob (job) | - | - | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | extensions.badOutput (write) | Video.increment | apply: Video.increment | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | extensions.twice (write) | Video.increment | apply: Video.increment | none |
| product/tests/fixtures/mysql-worker-e2e/app.aip | extensions.undeclared (write) | Video.increment | apply: Video.increment, Video.reset | undeclared apply Video.reset |
| product/tests/fixtures/mysql-worker-e2e/app.aip | extensions.view (write) | Video.increment | apply: Video.increment | none |
| prototype/example/app.aip | operations.textLength (read) | - | - | none |
| prototype/example/app.aip | operations.textLengthJob (job) | - | - | none |
| prototype/example/app.aip | extensions.confirm (write) | Event.mark | apply: Event.mark | none |
| spikes/spike-v1-fixture/fixture/recruitment.aip | extensions.stats (read) | Apply.approvedCount | aggregate: Apply.approvedCount | none |
| spikes/spike-v6-transport/tests/fixtures/audit.aip | operations.settle (job) | apply.Order.pay, read.Order | read: Order; apply: Order.pay | none |
| spikes/spike-v6-transport/tests/fixtures/audit.aip | extensions.payNow (write) | Order.pay | apply: Order.pay | none |
| spikes/spike-v6-transport/tests/fixtures/job-breaker.aip | operations.callout (job) | http.crm | - | none |
| spikes/spike-v6-transport/tests/fixtures/job-data.aip | operations.crashy (job) | apply.Order.pay, read.Order | - | function not found |
| spikes/spike-v6-transport/tests/fixtures/job-data.aip | operations.settle (job) | apply.Order.pay, read.Order | read: Order; apply: Order.pay | none |
| spikes/spike-v6-transport/tests/fixtures/job-data.aip | operations.sneaky (job) | read.Order | - | function not found |
| spikes/spike-v6-transport/tests/fixtures/job-retry.aip | operations.boom (job) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/job-retry.aip | operations.boomRetry (job) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/job-retry.aip | operations.exhausted (job) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/job-retry.aip | operations.flaky (job) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/job-retry.aip | operations.later (job) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/jobs.aip | operations.count (job) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/jobs.aip | operations.length (read) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/jobs.aip | operations.report (job) | events.notifications, files.reports, jobs.length | - | none |
| spikes/spike-v6-transport/tests/fixtures/jobs.aip | operations.slow (job) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/message-handlers.aip | operations.consume (job) | messages.orders, messages.partner | - | none |
| spikes/spike-v6-transport/tests/fixtures/operations.aip | operations.ctxProbe (read) | - | aggregate: Hidden.total | undeclared aggregate Hidden.total |
| spikes/spike-v6-transport/tests/fixtures/operations.aip | operations.forbidden (read) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/operations.aip | operations.invalid (read) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/operations.aip | operations.length (read) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/operations.aip | operations.ranged (read) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/operations.aip | operations.slow (read) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/operations.aip | operations.unsafeInteger (read) | - | - | none |
| spikes/spike-v6-transport/tests/fixtures/outbound-webhook.aip | operations.notify (job) | http.subscribers | - | function not found |
| spikes/spike-v6-transport/tests/fixtures/outbox.aip | operations.fulfil (job) | apply.Order.ship | - | function not found |
| spikes/spike-v6-transport/tests/fixtures/triggers.aip | operations.receive (job) | apply.Order.pay | - | function not found |
| spikes/spike-v6-transport/tests/fixtures/triggers.aip | operations.sweep (job) | apply.Order.pay, read.Order | - | function not found |
| spikes/spike-v6-transport/tests/fixtures/webhook-big.aip | operations.receive (job) | apply.Order.pay | - | function not found |
| spikes/spike-v6-transport/tests/fixtures/webhook-big.aip | operations.sweep (job) | apply.Order.pay, read.Order | - | function not found |
| spikes/spike-v6-transport/tests/fixtures/webhook-subscriptions.aip | operations.fanout (job) | apply.Subscription.disable, apply.Subscription.failed, apply.Subscription.recovered, http.subscribers, read.Subscription | read: Subscription; apply: Subscription.disable, Subscription.failed, Subscription.recovered | none |
| spikes/spike-v6-transport/tests/fixtures/write-extension-dx.aip | extensions.badFilter (write) | Event.mark, read.Event | - | implementation source not found |
| spikes/spike-v6-transport/tests/fixtures/write-extension-dx.aip | extensions.crashes (write) | Event.mark | - | implementation source not found |
| spikes/spike-v6-transport/tests/fixtures/write-extension-dx.aip | extensions.gate (write) | Event.other, read.Event | - | implementation source not found |
| spikes/spike-v6-transport/tests/fixtures/write-extension-dx.aip | extensions.keyed (write) | Event.mark | - | implementation source not found |
| spikes/spike-v6-transport/tests/fixtures/write-extension-dx.aip | extensions.noAccess (write) | Event.mark | - | implementation source not found |
| spikes/spike-v6-transport/tests/fixtures/write-extension-serializable.aip | extensions.run (write) | Event.mark | - | implementation source not found |
| spikes/spike-v6-transport/tests/fixtures/write-extension.aip | extensions.run (write) | Event.mark | - | implementation source not found |
| verification/1.0-consumers/apps/stockdesk-1.1.0-src/app.aip | extensions.place (write) | Order.confirm, Product.take, read.Order, read.OrderLine, read.Product | read: OrderLine, Product; apply: Order.confirm, Product.take | declared read Order not seen |
| verification/1.0-consumers/apps/stockdesk/app.aip | extensions.place (write) | Order.confirm, Product.take, read.Order, read.OrderLine, read.Product | read: OrderLine, Product; apply: Order.confirm, Product.take | declared read Order not seen |
| verification/1.0-consumers/apps/taskshare/app.aip | extensions.reschedule (write) | TodoItem.edit, read.TodoItem | read: TodoItem; apply: TodoItem.edit | none |

99 extension(s) in 35 definition(s)
