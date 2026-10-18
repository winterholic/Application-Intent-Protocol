# Slice13-D: home/category, home/personalized (WBS 7.2.3)

이 문서는 두 API 포팅의 **범위와 한계**를 적는다. 완전 parity나 WBS 완료를 주장하지 않는다. 실험 baseline이다.

원본: new-facereview `7948b7e721837fdb73c427cb54e12cad4ec2d5af`, `app/services/home_service.py` 251-439, `common/utils/recommendation_alg.py`.
파일: `extensions/slice13d_home.py`(두 확장), `extensions/recommendation_math.py`(순위 계산 포팅), `facereview.aip`의 `resource Video` 확장 선언 2개, 시험은 `product/tests/slice13d_home_*`와 `spikes/spike-v4-worker/tests/slice13d_home_native.rs`.

## 채택한 안 (A): 캐시 생산은 원본 Celery, AIP는 hit만

| | 원본 | 이 포팅 |
|---|---|---|
| 풀·카테고리 캐시 생산 | Celery beat `rebuild-recommendation-pool`이 30분(1800s)마다 `HomeService._build_and_cache_ranked_pool()` 호출 (`common/config/celery_config.py` 43-56, `common/tasks/scheduled_tasks.py` 24-27). Redis `facereview:reco:pool`(SET, TTL 7200s), `facereview:reco:category`(DEL, HSET, EXPIRE 7200s) | 같은 원본 Celery 작업이 계속 생산한다고 전제한다. AIP는 쓰지 않는다 |
| 읽기 hit | Redis GET/HGETALL | `ctx.redis.get/hgetall` (read-only capability) |
| 읽기 miss | 프로세스 내 `_MEM_CACHE` 확인 후 **동기 빌드**(MySQL+Mongo로 전 영상 base_score 계산, 부족 카테고리를 랜덤 영상으로 보충, Redis에 쓰고 MEM 갱신)해서 응답 | `SLICE13D.POOL_CACHE_MISS` / `SLICE13D.CATEGORY_CACHE_MISS` 명시 오류. **아무것도 빌드하거나 쓰지 않는다.** 빈 목록·샘플로 대체하지 않는다 |
| MEM fallback | 있음 | 옮기지 않음. 확장에는 지속 상태가 없다 |

원본과의 격차(의도된 것이며 숨기지 않는다):
- 원본은 캐시 miss에서도 요청이 성공한다. 이 포팅은 실패한다. 첫 기동, TTL 2시간 만료(Celery 중단), Redis flush 뒤에는 Celery가 다시 쓸 때까지 두 API가 오류를 낸다.
- 원본 miss 빌드가 하던 일(전 영상 점수 계산, 부족 카테고리 랜덤 보충, TTL 쓰기)은 이 포팅에 없다. 현재 capability로는 불가능하다: Redis 쓰기 명령 5종이 `WIRE.CAPABILITY_DENIED`(실행 확인), 영상별 `like_count`를 위한 전체 사용자 COUNT는 읽을 길이 없음(코드·정의 읽기, 미실행), Mongo는 호출당 100문서 상한. 쓰기 capability, READ effect 변경, 서비스 주체, jobs는 만들지 않았다.
- 순수 pool builder(원본 `_load_scored_videos` 등가)는 별도 worker가 별도 branch에서 구현한다. 이 branch는 그 파일을 소유하지 않는다.

## Redis 장애와 startup

- 런타임 Redis 장애: 원본은 `redis_client.get`/`hgetall` 호출을 감싸는 try/except가 없어 예외가 그대로 올라간다(코드 읽기, `home_service.py` 312-322, 325-347). 기획이 원본 실행으로 예외 전파를 실증했다고 전달받았다(이 Run은 원문을 읽지 못해 재현하지 않음). 이 포팅도 장애를 삼키지 않는다: 어댑터가 없거나 실패하면 `WIRE.ADAPTER_UNAVAILABLE`이 `WIRE.EXTENSION_ERROR "확장 실패: WIRE.ADAPTER_UNAVAILABLE"`로 전달된다(어댑터 없음은 실행 확인: `category_absent_adapter_and_cache_miss_are_explicit_errors`). 의미는 맞다. 다만 클라이언트에는 코드가 `WIRE.EXTENSION_ERROR`로 감싸여 보인다(D6).
- startup fail-fast: 설정된 어댑터가 연결·ping에 실패하면 서비스는 listening 전에 기동 실패한다(`product/MANAGED-EXECUTION.md` 서비스 설정 절). 이 포팅은 그 동작을 바꾸지 않았다. 어댑터를 설정하지 않은 경우만 호출 시점에 `ADAPTER_UNAVAILABLE`이다. 이 Run은 startup 경로를 실행하지 않았다(문서 인용).

## category 순서: 비보장 정보 (결함도, 동등성 실패도 아니다)

원본의 `get_videos_by_category_emotions` 응답 카테고리 순서는 Redis HGETALL이 준 순서이고, **Redis는 해시 필드 순서를 보장하지 않는다.** 이 문서는 순서 차이를 결함이나 원본 동등성 실패로 세지 않는다.

- 기획 재분류(전달받은 내용, 이 Run은 원문을 읽지 못해 재현하지 않음): 원본 해시 순서는 비보장이고 재시작 뒤 hashtable 순서가 달라지는 것이 실측됐다. 프론트 2종은 `category_name`으로 `find`해서 쓰며 내용은 일치한다.
- 이 Run의 관측(probe, 보장 아님): 작은 해시(16필드, listpack)는 실제 Redis 7.4.11에서 삽입 순서를 유지했다. 이 포팅의 드라이버 경로(`RedisDriverCapability` → worker)에서는 music, game, drama 순으로 넣은 해시가 drama, game, music(이름순)으로 도착했다. 코드 읽기상 드라이버가 HGETALL을 `HashMap`에 담아 `serde_json`(preserve_order 없음)으로 넘기기 때문이다.
- 이 이름순은 probe 근거의 추론이며 **새 계약으로 보장하지 않는다.** 아무도 이 순서에 의존하면 안 된다. 시험(`category_order_delivered_by_the_driver_path_is_unguaranteed_information`)은 바뀌면 알아채려는 관측 고정일 뿐이며, 같은 시험이 카테고리별 내용(영상 수)이 순서와 무관하게 맞음을 확인한다.
- `preserve_order`를 켜거나 임의로 정렬하지 않았다.

## Mongo projection: 원본 3필드 중 1필드만 제외 가능

원본 `find_recent_summaries_by_user_id`는 `emotion_score_timeline`, `most_emotion_timeline`, `client_info` 3개를 제외한다. 현재 드라이버는 projection 키를 허용목록 7개(`_id, video_id, user_id, dominant_emotion, created_at, emotion_score_timeline, total_frames`) 안에서만 받는다.

실행 확인(실제 mongod, `projection_excludes_only_one_of_the_original_three_fields`):
- 3필드 동시 제외, `{client_info:0}` 단독, `{most_emotion_timeline:0}` 단독은 모두 `WIRE.BAD_VALUE`로 거부된다.
- `{emotion_score_timeline:0}`만 허용되며, 이때 반환 문서에서 `emotion_score_timeline`은 빠지고 `most_emotion_timeline`과 `client_info`는 **그대로 worker로 전달**된다(작은 문서로 확인).
- 확장은 이 1필드만 제외한다.

경계(합성 크기, 실측 아님): 드라이버 결과 상한은 1 MiB다. 시험은 합성 문서(60,000자 `most_emotion_timeline` x 20, 1.1 MB 한 건)로 상한 초과 시 `BAD_VALUE`가 나는 메커니즘만 보인다. **실제 `youtube_watching_data` 문서 크기는 측정하지 않았으므로 실데이터에서 상한에 닿는다거나 장애가 난다고 단정하지 않는다.** 영향 판단에는 실제 문서 크기 분포 측정이 필요하다(미실측). 이 필드들의 개인정보 민감도도 확인하지 않았으므로 민감도에 대한 평가는 하지 않는다. 이 branch는 코어를 바꾸지 않는다(허용목록은 UUID 수정 worker 소유의 `capability.rs`).

## user_id (UUID)와 개인 프로필

FaceReview의 `user_id`는 MySQL과 Mongo 모두 UUID 문자열이다. 현재 `CapabilityPrincipal`은 i64 handle만 갖고 있어 private `youtube_watching_data`와 UUID 문서를 맞출 수 없다(실제 mongod에서 0건/`ACCESS_DENIED` 실증, 기획 확인). 별도 core worker가 수정한다. 확장은 입력의 userId를 신뢰하지 않고, 정책이 걸린 `User` 행의 `id`만 필터에 쓴다. 숫자 id로 통과하는 통합 시험은 "배관이 원본 순위를 낸다"만 보이며 **실제 UUID 사용자 프로필 동등성은 미검증**이다.

### 본인 식별 (admin 다중 User 가시성)

`User` 행 정책은 `(id = actor and active(actor)) or isAdmin(actor)`(`facereview.aip` 30행)라서 ADMIN/SUPER_ADMIN이 호출하면 `read User`가 여러 사용자를 돌려줄 수 있고, 필터·정렬 없는 `limit 1`의 첫 행이 본인이라는 보장이 없다. 이 추론은 reviewer가 제기했고 기획이 정의 코드로 대조했다. 이 Run은 정의의 정책 줄을 직접 읽어 확인했지만, **실제 MySQL·실제 admin·실제 플래너 SQL로는 실행하지 않았다**(scripted 다중행 시험만).

지원되는 self-selection 확인 결과:
- `read` 필터에 호출자를 가리키는 값이 없다(`id.eq`는 리터럴 값). worker `ctx`에도 공개 identity가 없다. 호출 input의 임의 UUID는 신뢰하지 않는다.
- 서버가 `user = actor`로만 거르는 자원(`VideoViewLog`, `UserFavoriteGenre`)의 행은 `user` 값이 곧 본인이다(서버 파생).

확장의 처리(`_resolve_self`):
- `User`를 `limit 2`로 읽는다. 1행이면 본인 행(회원은 본인만, admin도 자기 자신은 항상 보임)이고, own-only 행의 `user`와 모순되면 `SLICE13D.SELF_IDENTITY_MISMATCH`.
- 2행 이상이면 admin이다. own-only 행의 `user`가 정확히 하나면 그 값을 본인으로 쓰고, 없으면 `SLICE13D.SELF_IDENTITY_UNRESOLVED`, 둘 이상이면 `MISMATCH`. 추측한 id는 private Mongo 질의에 보내지 않는다.
- 한계(설계 의존): 시청 기록도 선호 장르도 없는 admin은 본인을 확정할 수 없어 오류가 난다. 이를 풀려면 "호출자 본인 id"를 서버가 확장에 주는 공개 경로(core)가 필요하며 이 branch는 새 API를 만들지 않았다.
- 증거: `product/tests/slice13d_home_extension_test.py`의 `SelfIdentity` 5건(scripted). 수정 전 코드(`bd0bd23`)에 같은 시험을 돌리면 3건 실패·1건 오류로, 스크립트된 admin 응답에서 Mongo에 **다른 사용자의 id**로 질의하는 것이 재현된다.

## 증거 구분

| 증거 | 내용 |
|---|---|
| 원본 실함수 oracle | 고정 SHA의 `recommendation_alg.py` 실행 대조 (`slice13d_home_oracle_test.py`) |
| scripted peer | 가짜 ctx. DB/드라이버/HTTP 증거 아님 (`slice13d_home_extension_test.py`) |
| 실제 드라이버 + worker | 전용 mongo/redis, Python worker, 실제 로더가 컴파일한 정의. MySQL은 scripted (`slice13d_home_native.rs`) |
| 실제 HTTP 서버 + worker | `spikes/spike-v6-transport/tests/slice13d_home_http.rs`: 실제 `POST /extension`, 계약 지문 검사, 토큰, 실제 정의. MySQL 행과 Mongo/Redis는 **scripted(메모리 capability)** 이라 드라이버 증거 아님 |
| 미검증 | 실제 MySQL, `service serve` 경유 E2E(시험은 `listen_relational_with_extensions`를 직접 사용), 원본 `HomeService` 실행, 실 UUID 프로필, `mysql_facereview_reads` |

## 알려진 코어 의존성

D1 `.aip`가 `read./mongo./redis.` grant를 선언 못 함(선언은 grant 없이 두고 시험이 붙임), D2 확장 출력이 스칼라뿐(응답이 JSON Text 하나), D3 UUID principal, D4 Mongo projection 허용목록(원본은 `emotion_score_timeline`, `most_emotion_timeline`, `client_info` 3개를 제외하나 1개만 제외 가능), D5 Redis 쓰기·like 전체 COUNT, D6 오류 코드 래핑(HTTP에서는 내부 코드가 아예 사라져 어댑터 없음·캐시 miss·권한 거부가 모두 같은 `WIRE.EXTENSION_ERROR "공식 읽기 확장 호출 거부"`로 보임, 실행 확인), D7 미선언 resource read 허용(HTTP 서버 경로에서도 `READ_OK` 실행 확인).
