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

## category 순서: 실측과 미실측

실측(이 Run, 실제 프로세스):
- 실제 Redis 7.4.11에서 16개 필드를 알파벳 순이 아닌 순서로 HSET하면 HGETALL이 **삽입 순서 그대로** 돌려준다(OBJECT ENCODING = listpack).
- 이 포팅의 서버 경로(실제 `RedisDriverCapability` → worker)로는 music, game, drama 순으로 넣은 해시가 **drama, game, music(이름순)** 으로 확장에 도착한다(`gap_category_order_follows_the_capability_map_not_the_redis_hash`). 코드 읽기상 드라이버가 HGETALL을 `HashMap`에 담아 `serde_json`(preserve_order 없음)으로 넘기기 때문이다. 확장은 받은 순서 그대로 응답한다.
- 원본 코드상 해시 삽입 순서는 `by_category`의 첫 등장 순서(base_score 내림차순 스캔에서 카테고리가 처음 나온 순서), 이어서 부족 카테고리 보충분이다(코드 읽기).

미실측 / 단정하지 않는 것:
- 원본의 카테고리 순서가 프론트나 공개 계약이 보장하는 것인지.
- 운영 Redis에서 해시가 항상 listpack(삽입 순서 유지)인지. Redis는 해시 필드 순서를 문서상 보장하지 않는다. 필드 수·값 크기가 임계치를 넘으면 hashtable 인코딩이 되어 순서가 달라질 수 있다(미실측).
- 기획 쪽 실증(삽입 순서 HGETALL 18/18, 정렬 16/18, 카테고리 순서 2건 차이)은 전달받은 수치이며 이 Run이 원문을 읽거나 재현하지 못했다.

순서를 맞추려고 `preserve_order`를 켜거나 임의로 정렬하지 않았다. 순서가 계약이면 코어가 해시를 순서 있는 쌍 목록으로 넘기는 변경이 필요하다(기획 결정).

## user_id (UUID)와 개인 프로필

FaceReview의 `user_id`는 MySQL과 Mongo 모두 UUID 문자열이다. 현재 `CapabilityPrincipal`은 i64 handle만 갖고 있어 private `youtube_watching_data`와 UUID 문서를 맞출 수 없다(실제 mongod에서 0건/`ACCESS_DENIED` 실증, 기획 확인). 별도 core worker가 수정한다. 확장은 입력의 userId를 신뢰하지 않고, 정책이 걸린 `User` 행의 `id`만 필터에 쓴다. 숫자 id로 통과하는 통합 시험은 "배관이 원본 순위를 낸다"만 보이며 **실제 UUID 사용자 프로필 동등성은 미검증**이다.

## 증거 구분

| 증거 | 내용 |
|---|---|
| 원본 실함수 oracle | 고정 SHA의 `recommendation_alg.py` 실행 대조 (`slice13d_home_oracle_test.py`) |
| scripted peer | 가짜 ctx. DB/드라이버/HTTP 증거 아님 (`slice13d_home_extension_test.py`) |
| 실제 드라이버 + worker | 전용 mongo/redis, Python worker, 실제 로더가 컴파일한 정의. MySQL은 scripted (`slice13d_home_native.rs`) |
| 미검증 | 실제 MySQL, 서비스 HTTP `/extension` E2E, 원본 `HomeService` 실행, 실 UUID 프로필, `mysql_facereview_reads` |

## 알려진 코어 의존성

D1 `.aip`가 `read./mongo./redis.` grant를 선언 못 함(선언은 grant 없이 두고 시험이 붙임), D2 확장 출력이 스칼라뿐(응답이 JSON Text 하나), D3 UUID principal, D4 Mongo projection 허용목록(원본은 `emotion_score_timeline`, `most_emotion_timeline`, `client_info` 3개를 제외하나 1개만 제외 가능), D5 Redis 쓰기·like 전체 COUNT, D6 오류 코드 래핑, D7 미선언 resource read 허용.
