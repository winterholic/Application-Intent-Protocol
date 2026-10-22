# 진행 현황과 쌓인 질문

## 재개 지점 (2026-10-22 밤, 7.2.9 요청 승인 뒤 기록)

7.2.9의 요청 승인을 durable job `approveVideoRequest`로 옮겼다(29ec998): 쓰기 확장이 전이만 부를 수 있어서 7.2.7의 job `create`·Mongo 쓰기를 썼고, `VideoRequest.accept` 전이는 요청의 `youtube_url`을 가진 `Video`가 있을 때만 가능하다. 원본 `AdminService.approve_video_request`와 영상 행·Mongo 문서 2개·요청 상태가 같았다(`reference/slice14b_oracle.py`). 7.2.9의 남은 것: `POST home/video/recommend`(요청 생성: 서버 파생 `youtube_full_url`과 목록 전체 중복 거절. 정의 언어에 create의 계산된 값이 없어 새 언어 기능, 또는 사용자가 부르는 job이 필요하고 job은 비동기라서 선택이 필요하다. 원본은 `video_request.youtube_url`에 유일 인덱스가 없어 경쟁 상태에서 중복을 막지 못한다), dashboard(가입 추이·WAU·완주율: 날짜 구간 집계와 Mongo), 시스템 상태(호스트 지표), 더미 데이터(Mongo 쓰기). 7.2.8의 남은 마이페이지 Mongo 읽기 6개는 현재 Mongo 읽기 capability가 막는다: 읽기 filter는 동등과 `$in`뿐(날짜 범위 `$gte/$lt`·증분 `finalized_at > 체크포인트` 불가), 문서는 호출당 최대 100개·응답 1MB, 읽기 확장은 ctx 호출 64회, 그런데 원본은 `emotion_score_timeline`(초당 2프레임 × 영상 길이)이 큰 문서를 200개~전체 읽어 집계한다. 권장: 7.2.3의 `video_emotion_timeline`처럼 사용자 본인 `youtube_watching_data` 위의 서버 파생 읽기 명령(압축 40버킷·감정 초 합계·캘린더 일별·피크 모먼트·DNA 통계)을 서버에서 계산하고 확장은 점수 규칙(`_determine_dna_type`·`_compute_traits` 등)과 캐시 표(`UserEmotionSummary`·`UserEmotionDna`) 갱신만 맡기는 설계다. 이것은 읽기 capability 계약의 확장이라 DECISIONS에 대안 비교와 함께 먼저 적어야 한다(이번에는 구현하지 않음). CI 주의: main push는 이전 Verify를 취소한다(concurrency). 연속 push 뒤 Verify 결과는 마지막 SHA의 것만 남으니 push 사이에 30분 정도 둔다. 1951b9f·24ddd30·67cc27d의 Verify는 취소됐고, 그 변경을 모두 포함한 29ec998은 Verify 39708253311을 포함해 7종이 success다(Linux worker jobs 39708253334, Database contracts 39708253401 등).

## 이전 재개 지점 (2026-10-22, 7.2.8 조각 A·B 뒤 기록)

7.2.7은 done이다(아래 절). 7.2.8(마이페이지·이메일·비밀번호·탈퇴·감정 집계)은 관계형·인증 부분을 두 조각으로 끝냈고 doing이다. 조각 A(1951b9f): 로그인 때 약한 해시 재해시(검사한 해시일 때만 갱신), `User.completeTutorial`·`User.updateProfile`·`UserFavoriteGenre` create/remove(선호 장르 교체는 `atomic` 묶음), `/auth/me`는 본인 `User`+`UserFavoriteGenre` 읽기, 탈퇴는 기존 `User.withdraw`이고 다음 reissue가 모든 세션을 끝낸다. 플랫폼 수정: `repeat unchanged` 판정에서 시계(`now`)로 정하는 필드를 뺐다(MySQL·PostgreSQL 쓰기 경로). 조각 B(24ddd30): `credentials.mail`로 `/auth/email/verification`·`/auth/email/verify`·`/auth/password/verification`·`/auth/password`(Bearer 필요, 번호·토큰은 `aip_auth_challenges`에 digest, 시도 5회 제한, 비밀번호 변경은 다른 세션 폐기), 결정 D-P-CREDMAIL. 남은 7.2.8: 마이페이지 Mongo 읽기 6개(`videos/recent`·`emotion/summary`·`highlight`·`emotion/calendar`·`moments`·`emotion-dna`, 원본 `MypageService` 라인 250~760과 `_DNA_TYPES`). `emotion/summary`는 GET 안에서 `UserEmotionSummary`를 낙관적 잠금(`lock_version`)으로 갱신하는 증분 집계라서 읽기 확장만으로는 안 되고 `UserEmotionSummary` 전이(또는 쓰기 확장)가 필요하다. 원본 대조는 `reference/slice13e_oracle.py`·`slice13f_oracle.py` 방식(원본 `MypageService`를 같은 MySQL·MongoDB에서 직접 실행)을 쓴다. 그 다음 7.2.9의 남은 부분이다. CI: 73e9470은 7종 success(Verify 39703774966). 1951b9f·24ddd30의 CI는 push 직후라 확인 전이다.

## 이전 재개 지점 (2026-10-22, 7.2.7 완료 뒤 기록)

7.2.7(예약 업무)은 원본 Celery beat 3작업(`rebuild_recommendation_pool`, `fetch_youtube_trending_videos`, `fill_youtube_category_videos`)이 모두 durable job으로 옮겨져 완료다(조각 A·B·C, 근거는 [제품 검증](../product/VERIFICATION.md)의 7.2.7 세 절). 조각 C(카테고리 보충)는 원본 `execute()`와 36행·72문서·quota 1212가 같았고 실서버 시험이 통과했으며, 장르 사이 quota 검사를 빼는 변이가 시험을 깨뜨렸다. 같은 커밋이 CI Verify 39664738540의 실패(풀 job 시험이 예약 run의 정산을 기다리지 않고 개수를 센 경합)를 고쳤다. 한계: 서버 설정 `limits.job_calls`를 1000 이상으로 올려야 한다, 트렌딩·보충의 예약(`schedules`)은 서버 설정이며 이 두 job만의 예약 시험은 하지 않았다(풀 job이 같은 경로를 보였다), 실제 YouTube credential이 없어 live smoke는 미실행이다. 다음은 7.2.8(인증 잔여: 약한 해시 재해시, `/auth/me`·`/auth/tutorial`, 탈퇴 때 세션 폐기, 마이페이지·이메일·비밀번호 변경·감정 집계), 이어서 7.2.9의 남은 부분(요청 승인은 job `create`와 Mongo 쓰기 capability가 이미 있어 쓰기 확장 쪽 `create` 호출만 필요한지 확인, dashboard, 시스템 상태, 더미 데이터)이다. 시험 환경 준비는 아래 7.2.7 조각 A 단락과 같다.

## 이전 재개 지점 (2026-10-22, 7.2.7 조각 A 뒤 기록)

7.2.7(예약 업무)은 조각 A(추천 풀 캐시의 예약 갱신)까지 끝났고 doing이다. 남은 것은 조각 B(YouTube 인기 영상 수집 `YoutubeTrendingJob`)와 조각 C(카테고리 보충 `YoutubeCategoryFillJob`)이며 둘 다 새 플랫폼 기능이 먼저 필요하다. (1) HTTP 연동이 JSON 본문 POST뿐이라 YouTube Data API의 GET 쿼리(`pageToken`·`id`·`q`, API key 쿼리 파라미터)를 부를 수 없다: 서버 설정에 허용 파라미터 목록과 비밀 쿼리 파라미터(`secret_query_env`)를 두고, 확장은 허용된 스칼라 값만 주는 방식을 권한다(URL·메서드·키는 서버가 정한다는 기존 원칙의 연장). (2) job ctx가 전이만 부를 수 있어 새 `Video` 행(원본 `_save_video`)을 만들 수 없다: job의 `create` capability가 필요하고 7.2.9의 요청 승인(영상 생성과 `video_distribution` 초기 문서)도 같은 기능을 쓴다. Mongo 초기 문서는 이번에 연 job Mongo 쓰기(`update_one` + `$setOnInsert`, 반복 가능)로 된다. (3) 중복 확인용 `Video` 읽기 필터 `youtube_url.in`과 카테고리별 개수 집계가 정의에 없다. 원본 대조는 원본 job 클래스의 `_map_youtube_category`·`_classify_category_by_keywords`와 `requests.get`을 로컬 가짜 YouTube로 돌린 `execute()` 결과(Video 행과 Mongo 문서)를 쓰는 방식이 가능하다(`reference/slice14a_oracle.py`가 같은 원본 로딩을 이미 한다). 시험 환경은 이전 단락에 더해: 인증 있는 MongoDB 7(`mongo:7`, `MONGO_INITDB_ROOT_USERNAME=aip_test_slice13a`·`MONGO_INITDB_ROOT_PASSWORD=local_fixture_only_13a`, 15441, URL에 `?authSource=admin`), 별도 mongod(25443), Redis 7.4(15442), `scripts/mysql-live-fixture.sh up`(`AIP_FIXTURE_TAG`로 컨테이너 이름 지정), PostgreSQL 17(5433, `POSTGRES_USER=runner`·`POSTGRES_HOST_AUTH_METHOD=trust`)과 `AIP_TEST_DATABASE_URL`·`AIP_TEST_ADMIN_URL`. 원본 대조용 venv는 `python3 -m venv` 뒤 Flask 3.0.2·Werkzeug 3.0.2·SQLAlchemy 2.0·Flask-SQLAlchemy 3.1.1·PyMySQL·pymongo·redis·cryptography·PyJWT·flask-socketio·flask-cors·celery·flask-smorest·marshmallow·isodate·requests를 설치하고 `/tmp`가 아닌 디렉터리에서 실행한다. 그 다음은 7.2.8(인증 잔여)과 7.2.9의 남은 부분이다.

## 2026-10-22 WBS 7.2.7 조각 A: 추천 풀 캐시의 예약 갱신과 durable job의 Mongo·Redis 쓰기 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.7은 진행 중)

원본 Celery beat 작업 `rebuild_recommendation_pool`(30분 주기)을 durable job `rebuildRecommendationPool`로 옮겼다. 플랫폼에 job의 `access mongo.<collection>`·`access redis "<키>"` 쓰기(effect 장부에 key로 남고 재시도는 재생, 응답을 잃은 쓰기는 다시 보내도 같은 데이터가 되는 명령만 같은 key로 다시 보냄, job에는 saga 없음)와 서버 설정 `limits.job_calls`(기본 64)를 더했고 결정 근거는 [DECISIONS](../docs/DECISIONS.md)의 D-P-JOBSTORE다. 읽기 확장은 캐시를 먼저 읽고 비면 요청 안에서 풀을 만든다. 원본 `_build_and_cache_ranked_pool`과 같은 데이터에서 풀 19개·카테고리 6개·순서·만료가 모두 일치했다. 시험이 찾은 것: `Video` 읽기의 offset 비용 때문에 읽기 확장의 "5000개 상한"이 실제로는 약 800개였다(`cursor`를 선언해 keyset으로 읽게 고침), 읽기 확장의 폴백은 64 ctx 호출이라 약 3000개에서 실패한다, 서버가 job을 서빙하지 않을 때도 계약 지문을 job 포함으로 출력하던 기존 결함(`/read`가 `CONTRACT_MISMATCH`). 명령·결과·변이 확인·한계는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 조각 A 절이다.

## 이전 재개 지점 (2026-10-22, 7.2.5 완료 뒤 기록)

7.2.5(Mongo·Redis capability)가 done이다. 조각 A(`GET watch/video`, cee2d8a)와 조각 B(Mongo·Redis 쓰기 capability와 saga, c621143) 모두 main에 push됐고 Linux CI Verify가 success다. 다음은 7.2.7(예약 업무: 추천 풀 Redis 쓰기는 `set_ex` 1MB·`replace_hash_ex`로 가능, 쓰기 확장이 아니라 durable job의 ctx에 Mongo·Redis 쓰기를 열어야 하는지부터 확인), 7.2.8(마이페이지·이메일·비밀번호·탈퇴: 인증 코드와 재설정 토큰은 Redis `set_ex`·`getdel`, 메일은 외부 provider), 이어서 7.2.9의 남은 부분(요청 승인은 `video_distribution` 초기 문서 쓰기, 지표는 `incr_expire`·`push_trim_expire`·`lrange`, dashboard는 날짜 구간 집계)이다. 시험 환경 준비는 아래 단락과 같고 새로 필요한 것: Redis 7.4(`docker run --rm -p 127.0.0.1:15442:6379 redis:7.4 redis-server --requirepass local_fixture_only_13a`), 인증 있는 MongoDB 7(15441), 별도 mongod(25443, `AIP_TEST_MONGO_UUID_URL`), 그리고 spike-v4-worker 테스트가 쓰는 PostgreSQL(`AIP_TEST_DATABASE_URL`은 5433에 `--rm` postgres:17). 전체 검증은 `tools/verify.sh`의 cargo 단계를 같은 순서로 돌린 스크립트로 18단계 통과를 확인했다. 스크립트에 `PGPORT`를 내보내면 DB 가드가 거절한다. 주의: 이 환경의 훅이 heredoc 안의 백틱을 "명령 이름을 치환으로 구성"으로 막으니, 백틱이 든 문서·코드는 Edit·Write 도구로 쓴다.

## 이전 재개 지점 (2026-10-21 밤, 7.2.9 착수 전 기록)

7.2.2·7.2.3 완료(main에 push, CI success). 7.2.9(관리자, doing)는 읽기 조각(회원·요청·영상·댓글 목록 4개, 원본 `AdminService`와 62/62 일치)까지 구현했고 아래 "WBS 7.2.9 관리자 목록 읽기" 절과 [제품 검증](../product/VERIFICATION.md)에 기록했다. 남은 7.2.9: 요청 승인, dashboard, 시스템 상태, 더미 데이터(아래 막힌 것). 원본 `AdminService`를 같은 데이터에서 나란히 돌려 대조하는 `slice13e_oracle.py`(`slice13d_oracle.py`의 배포·원본 로딩을 재사용) 방식이 dashboard 읽기에도 쓰인다(venv·컨테이너 준비법은 아래). 막힌 것: 요청 승인(영상 생성과 Mongo 초기 분포 쓰기: 쓰기 확장은 전이만 부를 수 있고 Mongo는 읽기 capability뿐, 7.2.5), 더미 데이터(Mongo 쓰기), 시스템 상태(호스트 지표), dashboard(기간별 가입 추이·WAU·완주율은 날짜 구간 집계와 Mongo 필요, 7.2.5·7.2.7), `POST home/video/recommend`(7.2.3에서 넘어옴). 선행 7.2.5·7.2.7·7.2.8이 todo라서 7.2.9를 완료로 세려면 이들이 먼저다. 시험 준비: 원본 체크아웃(`~/development/projects/facereview/new-facereview`)은 CRLF라서 모든 파일이 수정으로 보이지만 내용은 같다(오라클이 `--ignore-cr-at-eol`로 판정). 원본 서비스를 돌릴 venv는 `python3 -m venv` 뒤 `Flask==3.0.2 Werkzeug==3.0.2 flask-socketio flask-cors SQLAlchemy>=2.0.36,<2.1 Flask-SQLAlchemy==3.1.1 PyMySQL pymongo cryptography celery redis flask-smorest marshmallow PyJWT`를 설치하면 된다(`/tmp`에서 실행하면 `/tmp/numbers.py`가 표준 라이브러리를 가리니 다른 디렉터리에서 실행). MySQL 8.4는 `scripts/mysql-live-fixture.sh up`(태그·포트는 환경 변수), MongoDB는 `docker run --rm --tmpfs /data/db mongo:8.0`.

## 2026-10-22 WBS 7.2.5 조각 B: Mongo·Redis 쓰기 capability와 외부 효과 saga (구현, 로컬 실DB·실서버 통과)

쓰기 확장이 관계형 트랜잭션 옆에서 MongoDB·Redis에 쓰고, 트랜잭션이 커밋되지 않으면 되돌리는 경로를 만들었다(원본 `common/saga`의 의미). 문법 `access redis "키 또는 접두어:*"`(읽기·쓰기 확장)와 쓰기 확장의 `mongo.<collection>`, 쓰기 명령(Mongo 4개, Redis 8개와 읽기 `lrange`), `compensate=True`가 남기는 보상 기록, `ExternalEffects`(효과 순서·최신 단계부터 보상·원본 모양의 `saga_transaction_log`), 커밋 불명은 보상하지 않고 `unknown`, 보상 실패는 새 코드 `WIRE.COMPENSATION_FAILED`, 설정 `workers.mongo.write_collections`·`ensure_indexes`가 들어갔다. 실제 MongoDB·Redis·MySQL·`aip service serve`로 성공·재생·확장 실패·검사 실패·Mongo 실패·Mongo 불통·Redis 불통·커밋 응답 유실을 확인했고 롤백 호출을 빼는 변이가 시험을 깨뜨리는 것도 확인했다. 한계: 프로세스가 커밋 전에 죽으면 `in_progress` log만 남고 자동 복구는 없다, Node worker 새 메서드는 문법 검사만 했다. 명령·결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 조각 B 절이다. 7.2.5의 나머지는 없고(원본 프레임 쓰기 흐름은 7.2.6, 인증·이메일 Redis 사용은 7.2.8, 추천 풀 캐시 갱신은 7.2.7이 이 capability를 쓴다), Linux CI는 c621143의 Verify(39650801406)가 `mysql_facereview_watch`·`mysql_external_effects`를 포함해 success다. 그래서 7.2.5를 done으로 센다.

## 2026-10-22 WBS 7.2.5 조각 A: `GET watch/video`와 조회수 전이 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.5는 진행 중)

7.2.3이 넘긴 `GET watch/video`를 옮겼다. Mongo capability에 파생 컬렉션 `video_emotion_timeline`(서버가 전 시청자 타임라인을 합산·100버킷화해 평균만 돌려주고 원본 세션은 열지 않음)을 더했고, `Video.watchDetail` 읽기 확장과 `Video.view` 전이(원본이 GET 안에서 하던 `view_count += 1`을 호출자가 읽기 뒤에 명시 호출)를 정식 정의에 선언했다. 정의 언어가 증감 대입을 null이 아닌 Int에만 허용해서 `db-defaults.sql`이 `video.view_count`를 NOT NULL로 바꾼다(원본은 NULL을 쓰지 않는다). 같은 데이터에서 원본 `get_video_detail`과 42/42 일치했다(영상 12 x 호출자 3 + 쓰기 6). 첫 실행의 3건 불일치(48.1 대 48.2)는 Mongo `$sum`의 보정 합산이 원인이어서 서버 쪽 합산을 원본과 같은 순서의 단순 합으로 바꿔 풀었다. 명령·결과·변이 확인은 [제품 검증](../product/VERIFICATION.md)의 2026-10-22 절이다. 7.2.5의 나머지(Mongo·Redis 쓰기 capability, 다중 저장 보상, TTL·네임스페이스)는 아직이다.

## 2026-10-21 WBS 7.2.9 관리자 목록 읽기 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조와 Linux CI 통과, 7.2.9는 미완)

원본 `AdminService.get_users·get_video_requests·get_videos·get_comments`를 정식 정의의 읽기로 옮겼다. `User`의 행별 집계 `watchCount`·`commentCount`, 관리자 전용 `totalCount`(`User`·`VideoRequest`·`Comment`), `name.like`·`email.like`·`content.like` 필터, `VideoRequest`·`Comment`의 연관 행 요청을 더했고, 같은 데이터에서 원본 `AdminService`를 직접 실행해 62개 사례가 모두 같았다(관리자 2명 x 29상황과 일반 회원 거절 4건). 알려진 차이는 하나다: 삭제됐거나 NULL 플래그인 영상은 `Video` 행 정책이 관리자에게도 숨기므로 그 영상 댓글의 `video_title`이 null이고 `video` 필터로 그 댓글을 찾을 수 없다(원본은 보여 준다). 요청 승인(영상 생성과 Mongo 쓰기), dashboard, 시스템 상태, 더미 데이터와 `POST home/video/recommend`는 아직 없다. 명령·결과·변이 확인은 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.3 추천 목록 포팅과 정식 정의 통합 (구현, 로컬 실DB·실서버·원본 서비스 대조와 Linux CI 통과)

원본 `home/category`·`home/personalized`·`watch/recommended`를 읽기 확장으로 옮겼다. 정식 `facereview.aip`가 이제 홈·시청 목록 확장 6개(검색·전체·북마크 3개와 추천 3개)를 Mongo 권한과 함께 직접 선언하고 `aip service serve`가 바로 서빙한다. 이를 위해 제품에 세 가지를 더했다: 읽기 확장의 `access mongo.<collection>` 선언, `Video`의 공개 `likeCount`·`commentCount` 집계, `youtube_watching_data` 프로젝션의 `emotion_percentages` 허용. 실제 서버와 원본의 `HomeService`·`WatchService`를 같은 MySQL 8.4·MongoDB 데이터에서 나란히 돌려 41개 답이 모두 같았고(같은 명령 3회 golden 동일), CI는 그 golden을 새 배포에 재생한다(`mysql_facereview_recommend`). 시험이 찾은 원본의 숨은 의미: 선호 장르 순위는 사용자가 고른 순서가 아니라 `(user_id, genre)` 유일 인덱스가 돌려주는 ENUM 선언 순서다. 명령·결과·변이 확인·한계는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다. Redis 풀 캐시는 쓰지 않고 요청마다 풀을 만든다(오프라인 갱신 7.2.7). **옮기지 못한 둘**: `GET watch/video`(전 시청자 타임라인 집계는 본인 세션만 주는 Mongo capability가 막고, GET 안의 조회수 증가는 쓰기 확장이 전이만 부를 수 있어 못 묶는다)는 7.2.5로, `POST home/video/recommend`(서버 파생 `youtube_full_url`, 전역 중복 거절, 한 트랜잭션)는 계산된 기본값과 `video_request.youtube_url` 유일 인덱스가 필요해 7.2.9로 넘겼고 이 항목의 완료로 세지 않는다.

## 2026-10-21 WBS 7.2.2 비밀번호 로그인·가입·재발급·로그아웃 (구현, 로컬 실DB·HTTP와 Linux CI 통과)

MySQL 서비스가 외부 IdP 없이 FaceReview 계정으로 로그인할 수 있게 했다. 서버 설정의 `credentials`를 켜면 `/auth/check-email·signup·login·reissue·logout`이 생긴다. 비밀번호는 공식 Python 확장(`product/providers/password_auth.py`, Werkzeug 3.0.2 형식)을 샌드박스 worker에서 실행해 확인하고, 서버가 읽은 해시는 확장 밖으로 나가지 않는다. 가입한 계정과 원본이 만든 계정은 첫 로그인 때 서버가 계정 행의 UUID로 actor에 연결하며, 클라이언트는 role·owner·id를 정할 수 없다(알 수 없는 키는 422, role은 설정의 기본값). 접근 토큰은 서버가 서명한 `at+jwt`(300초)이고 같은 검증기를 통과하며 로그인 세션이 살아 있어야만 받는다. refresh token은 단회용이다. 원본은 이미 쓴 refresh token을 다시 받아 새 접근 토큰을 발급했지만(고정 소스와 실제 API로 확인한 보안 결함) 이 포팅은 재사용을 감지해 세션 전체를 폐기한다. 로그아웃·정지 계정·운영자의 `principal revoke`가 쿠키와 접근 토큰을 함께 막는다. 시험은 실제 `aip service` 바이너리, 원본 11개 표 스키마의 격리 MySQL 8.4, Seatbelt 안의 Python worker에서 한 건(`mysql_facereview_auth`)이 통과했고 세션 저장소는 MySQL 8.4와 MariaDB 11.4에서 각 4건이 통과했다. 명령·결과·변이 확인·한계(로그인 시도 제한, 재해시, `/me`·`/tutorial`, 브라우저 쿠키 실행, Linux CI)는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다. 동시 가입 시험이 찾은 `principal bind` gap lock 교착(3회 중 1회)을 고쳤고 연속 8회 통과했다. Linux CI는 `673ee0e`의 Database contracts·Deployment checks와 `30ad7b2`의 Verify(Bubblewrap 안 Python worker와 이 인증 E2E 포함) 등 7종이 모두 success다. 로그인 시도 제한(proxy 책임)·약한 해시 재해시·`/auth/me`·`/auth/tutorial`·탈퇴 때 세션 폐기는 7.2.8, 브라우저의 교차 사이트 쿠키 실행과 화면 문구 분기는 7.2.10으로 넘겼고 이 항목의 완료로 세지 않는다. 이 절은 7.2.1.3의 제외 범위(SSE·WebSocket·`events_days`, `file_mounts`·`/files/mount-get`·`objects`, 명시 handler owner, 일부 실DB 경계 시험, MariaDB 서빙)를 지우지 않으며 7.2.6·7.2.10·7.2.11에서 추적한다. PostgreSQL `relay_messages` 재전달 문제는 코드 대조에 따른 추정이고 PostgreSQL 실행은 하지 않았으므로 측정한 결함으로 주장하지 않으며 별도 추적을 유지한다.

## 2026-10-21 WBS 7.2.1.3 MySQL 관리 파일 저장소 capability (구현, Linux CI 통과)

`access files.<bucket>`과 클라이언트 경로 `/files/put·get·delete`를 MySQL에 연결했다. 저장소는 `aip_managed_files` 한 표다(PostgreSQL의 `aip_files`와 이름을 나눴다). 부트스트랩과 `adopt-external --apply` 재실행이 만들고, `workers.enable_file_uploads`이거나 job이 `files.*`를 선언한 서비스만 시작 때 표를 검사한다. 이제 `enable_file_uploads` 설정을 거절하지 않는다. 바이트는 표에 LONGBLOB로 1MiB까지 두고, 핸들은 불투명 UUID이며 경로나 URL은 노출하지 않는다. 같은 이름에 같은 바이트·형식·만료를 다시 넣으면 같은 파일이고 다르면 충돌이다. 읽기는 owner·bucket·만료를 호출마다 데이터베이스 시계로 다시 검사한다. owner당 100개·10MiB 한도를 `aip_tx_locks` 버킷 잠금 아래에서 검사하고(만료된 파일은 세지 않고 이미 있는 이름의 재전송은 새 파일로 세지 않는다), job의 파일은 run과 effect로 범위를 좁혀 재시도가 같은 파일로 수렴하며 run 기한 하루 뒤에 만료된다. job의 put은 effect 장부와 같은 fenced transaction에 커밋되고 get은 장부 없이 읽는다. 만료 파일은 supervisor의 30초 정리 주기에서 1000건씩 최대 10배치 지운다. `file_mounts`(HTTP로 파일을 서빙하는 mount)와 `/files/mount-get`, 객체 저장소(`objects`)는 연결하지 않았다. 이 증분으로 7.2.1.3의 메시지·파일·이벤트 저장소 capability가 모두 MySQL에 연결되었다. Linux CI는 main `a0daf81`의 Verify 39577769052(실제 Node·Python worker E2E 포함), Database contracts 39577768890, Linux worker jobs 39577768904 등 8종이 모두 success다. 이 실행이 이벤트·메시지·메시지 핸들러·관리 파일 시험을 MySQL 8.4와 MariaDB 11.4 CI 컨테이너에서, worker E2E를 Linux Bubblewrap에서 돌렸다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL 메시지 핸들러와 만료 메시지 정리 (구현, Linux CI 통과)

`message_handlers`를 MySQL에 연결했다. 서버가 설정된 큐의 준비된 메시지를 서비스 principal의 job으로 바꾸고, job 접수와 메시지 ack가 같은 fenced transaction에 커밋된다. job 키는 메시지 id라서 재전달은 같은 job으로 수렴한다. 접수가 거부되면 nack로 백오프를 거쳐 배달 횟수를 다 쓰면 dead letter가 되고, 서비스 principal이 비활성이면 메시지를 클레임하지도 않아 다시 바인딩할 때까지 준비 상태로 남는다. 시험에서 PostgreSQL 경로와 같은 결함을 찾았다. job 입력에 `deliveries`가 들어 있어서 접수와 ack 사이에 서버가 죽었다가 메시지가 재전달되면 입력이 달라 `IDEMPOTENCY_MISMATCH`가 되고, 이미 만들어진 job이 있는데도 메시지가 nack되어 결국 dead letter로 간다. MySQL 경로는 같은 키의 job이 이미 있으면 메시지를 ack하고 넘어가도록 고쳤다. PostgreSQL 경로(`spikes/spike-v6-transport/src/durable.rs`의 `relay_messages`)는 고치지 않았고 같은 결함이 남아 있다고 본다(코드 대조만 했고 PostgreSQL 실행은 하지 않았다). 만료된 메시지는 supervisor의 30초 정리 주기에서 1000건씩 최대 10배치 지운다. 파일 저장소 capability가 남아 7.2.1.3은 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL job 메시지 큐 capability (구현, Linux CI 통과)

`access messages.<queue>`와 클라이언트 경로 `/messages/enqueue·claim·ack·nack·dead·redrive`를 MySQL에 연결했다. 저장소는 `aip_message_queue` 한 표다(PostgreSQL의 `aip_messages`와 헷갈리지 않게 이름을 나눴다). 부트스트랩과 `adopt-external --apply` 재실행이 만들고, `messaging.enabled`이거나 job이 `messages.*`를 선언한 서비스만 시작 때 표를 검사한다. 이제 `messaging` 설정을 거절하지 않는다. 발행은 owner당 1000건·64MiB 한도를 `aip_tx_locks` 버킷 잠금 아래에서 검사하고(이미 있는 키의 재전송은 한도를 건너뛴다), 같은 키·같은 본문은 같은 메시지이며 다른 본문은 충돌이다. job의 발행은 effect 장부와 같은 fenced transaction에 커밋되어 재시도가 같은 메시지를 재생한다. 클레임은 `SKIP LOCKED`로 후보를 고르고 조건부 갱신으로 가져가며(`RETURNING`이 없어 토큰으로 다시 읽는다), ack·nack는 살아 있는 임차권만 커밋하고 배달 횟수를 다 쓰면 dead letter가 된다. 만료된 임차권은 다음 클레임 때 READY나 DEAD로 돌아가고 redrive·만료 정리도 같은 계약이다. 클라이언트의 발행·소비는 설정의 `client_publish`·`client_consume`이 따로 켜야 하고 job이 선언하지 않은 큐는 `NOT_EXPOSED`다. `message_handlers`(메시지를 job으로 중계)와 만료 메시지 자동 정리 루프는 아직 연결하지 않았다. 파일 저장소가 남아 7.2.1.3은 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL job 이벤트 스트림 capability (구현, Linux CI 통과)

`access events.<stream>`을 MySQL job에 연결했다. 이벤트 저장소는 `aip_event_streams`·`aip_event_log`·`aip_event_keys` 세 표다(`aip_events`는 쓰기 펜스 표로 이미 있어 `aip_event_log`로 이름을 나눴다). 부트스트랩과 `adopt-external --apply` 재실행이 만들고, job이 `events.*`를 선언한 계약만 서비스 시작 때 표 존재·엔진을 검사한다. 발행은 스트림 행 잠금으로 순서를 매기고 같은 키·같은 본문은 같은 cursor로 수렴하며 다른 본문은 충돌이다. job의 발행은 effect 장부와 같은 fenced transaction에 커밋되어 재시도가 같은 cursor를 재생한다. 클라이언트는 `/events/read`로 자기 owner의 스트림을 읽고, job이 선언하지 않은 스트림은 `NOT_EXPOSED`다. SSE·WebSocket 구독과 보존 일수(`events_days`) 정리는 연결하지 않았다. 같은 작업에서 outbox relay가 다른 relay가 막 커밋한 행을 다시 잡아 영수증 삽입이 중복 키로 실패하는 기존 경쟁(12회 중 1회)을 `INSERT IGNORE`와 실제 삽입 행 수 계산으로 고쳤다. 메시지·파일 저장소 capability가 남아 7.2.1.3은 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL job 외부 HTTP capability (구현, Linux CI 통과)

`access http.<name>`을 MySQL job에 연결했다. 서버 시작 시 선언된 연동이 `workers.integrations`·`outbound_webhooks`에 없으면 거부한다. 호출은 PostgreSQL 경로와 같은 규칙으로 effect 장부에 남긴다. 의도(PENDING)를 먼저 커밋한 뒤 요청을 보내고 답을 같은 fenced transaction에서 기록한다. 같은 effect로 재시도하면 기록된 답을 재생하고 다시 보내지 않는다. 요청이 확실히 나가지 않은 경우에는 의도를 지워 같은 키로 다시 보낼 수 있게 하고, 나갔는데 답을 잃은 경우는 UNKNOWN으로 남겨 반복하지 않는다. outbound webhook은 수신 쪽이 webhook-id로 중복을 걸러내므로 결과가 불명이어도 같은 id로 재전송한다. 실제 MySQL 8.4 단위 시험과 Node·Python 실제 worker E2E(첫 시도가 재시도를 요청하고 두 번째 시도가 같은 호출을 재생, peer 요청 1건)가 로컬에서 통과했다. Push 뒤 main bb6cc09의 Verify·Database contracts·Linux worker jobs 등 8종이 모두 success다(Verify 39451511559, Database contracts 39451511837). 메시지·파일·이벤트 저장소 capability가 남아 7.2.1.3은 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL 트리거(schedule·webhook·outbox)와 job 교착 수정 (구현, Linux CI 통과)

`schedules`·`webhooks`·`outbox_handlers`를 MySQL job 저장소에 연결했다. 접수는 설정의 service principal로 같은 fenced transaction에서 사용자 enqueue와 같은 검사를 거친다. 서버가 만드는 키와 트리거 전용 연산은 사용자가 못 쓰게 막았고(MySQL 경로에는 키 접두사 검사만 있었다), outbox 영수증은 job 접수와 같은 트랜잭션에 쓴다. `message_handlers`는 계속 거부한다. 같은 작업에서 E2E를 반복하다 `/jobs/status`가 간헐적으로 `WIRE.DB_UNAVAILABLE`을 내는 기존 결함을 찾았다. MDL 고정 쿼리 `SELECT 1 ... LIMIT 1`이 직렬화 격리에서 인덱스 첫 레코드에 공유 락을 잡아 job claim·settle UPDATE와 교착했다. `WHERE 1=0`으로 바꿔 행 락 없이 MDL만 잡게 했다(InnoDB 교착 로그로 원인 확인, 새 시험은 옛 쿼리에서 실패). 로컬은 macOS Seatbelt에서 Node·Python 실제 worker로 확장 E2E를 4회 통과했다. Linux CI와 MariaDB 쪽 고정 쿼리 회귀는 CI 결과를 기준으로 한다. WBS는 메시지·파일·이벤트·외부 HTTP capability가 남아 완료로 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-20 WBS 7.2.1.3 HTTP job·관리 효과 연결 (구현, Linux CI 통과)

MySQL `/jobs/enqueue`·`status`·`result`·`cancel`을 제품 principal·계약 지문 검사 뒤에 연결했다. supervisor가 기존 `MysqlJobStore`에서 claim하고 실제 worker를 실행하며 heartbeat·재시도·취소·종료를 관리한다. 데이터 읽기·집계와 전이, 중첩 job은 선언한 capability만 쓴다. 전이·감사·effect 영수증은 같은 fenced transaction에 커밋하며, stale epoch와 다른 요청의 같은 effect key는 거부한다. 기존 배포는 migration 계정의 `adopt-external --apply` 재실행으로 AIP effect journal을 준비한다. runtime은 DDL을 실행하지 않고 영수증의 InnoDB·복합 기본키·필수 열을 시작 전에 검사한다.

Codex macOS 명령 환경에서 실제 MySQL HTTP·callback journal 통합 1 passed, MySQL/MariaDB 저장소 각각 3 passed, 기존 MySQL 쓰기 8 passed, 채택·JWT·기존 배포 journal 추가 2 passed다. 통합 시험의 dispatcher는 명시적으로 멈춰 두고 callback을 직접 실행하므로 실제 worker 실행 증거로 쓰지 않는다. 실제 HTTP→Node/Python job·재시도 중 effect 재생·중첩 job·실행 중 취소는 기존 Linux CI의 CLI E2E에 추가했고 Verify 39438678105에서 Node·Python 양쪽 모두 통과했다(`http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction` ok. 같은 키 수렴, 재시도 attempt 2에서도 effect 1회, 중첩 effect 2건, 지연 취소, 실행 중 취소 포함). WBS는 완료로 닫지 않는다. 메시지·파일·이벤트·HTTP 외부 capability와 trigger 연결, MySQL job 전용 OTLP span은 이 증분에서 검증하지 않았다. 아래 10월 18일의 미연결 설명은 당시 상태다.

## 2026-10-18 독립 검증 반영: spike clippy, S3 늦은 PUT 보수화, 의존성 (Claude 구현, 독립 검증 대기)

spike-v6 `items after a test module`와 v1·v2 clippy를 고치고 `tools/verify.sh`가 workspace 밖 engine crate를 lint하게 했다. 서명 PUT의 첫 `HEAD` 부재가 예약을 풀던 경로를 "보류 후 한 번 더 확인"으로 바꿨고(`settling`), 두 번째 확인 뒤 도착하는 전송과 장부 이전 객체는 여전히 세지 못한다. rustls 0.23.46으로 올려 `tools/dependency-check.sh`가 통과한다. 전체 게이트는 이 증분 뒤 SHA에서 다시 돌리지 않았고 마지막 성공은 `a97f1b9`다. 자세한 명령·결과·한계는 [릴리스 게이트](../product/RELEASE-GATE.md)의 "후속 증분"을 따른다. 독립 검증 전이므로 8.5·8.7을 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.7 main 게이트 실행 (Claude 실행, 독립 검증 대기)

다른 트랙이 fmt·clippy를 정리한 뒤 main `a97f1b9`를 고정해 `scripts/release-gate.sh --baseline 4c27040`을 실행했고 모든 단계가 통과했다(Rust 1212 passed). 앞선 후보 `4e78ddb` 결과는 별개로 남겼고 main 성공으로 바꿔 적지 않았다. S3 quota의 65분 이후 미계상 경계는 하드 상한이 아님을 시험으로 고정하고 영향을 [릴리스 게이트](../product/RELEASE-GATE.md)에 적었다. 인계 목록도 같은 문서에 있다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.10 독립 검증 결함 6건 수정 (Claude 구현, 독립 검증 대기)

nbf 선택 claim 허용과 실제 Keycloak smoke, IdP smoke 17번째 확인의 본문 검사, Python 코드젠의 3.9/3.10 import, job span 부모 연결과 실제 경로 시험, poc-runtime all-targets clippy, prototype smoke 전용 DSN 가드를 고쳤다. 명령과 결과, 확인하지 못한 것은 [제품 검증](../product/VERIFICATION.md)의 WBS 8.10 절을 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.3 FaceReview MySQL 읽기 포팅과 어댑터 결함 수정 (Claude 구현, 독립 검증 대기)

원본 FaceReview의 실제 MySQL 스키마(SQLAlchemy가 만든 11개 표)에서 9개 resource를 업무 DDL 0건으로 adopt하고 `aip service serve`로 읽었다. 원본 앱의 SQLAlchemy 쿼리와 같은 질문 21개(검색, 카테고리, 최신순·NULL 플래그 제외, offset 페이징, 댓글 가시성, 본인 전용 표, 필드 값)가 모두 같은 결과를 냈다. 원본은 `created_at` 등을 Python에서 채우므로 표에 DB default가 없고, 그래서 `default database` 필드는 DBA 단계(`db-defaults.sql`) 전에는 adoption이 이름을 대며 거절한다. 어댑터 결함 둘을 고쳤다: ENUM 값을 소문자로 비교해 `'GENERAL'` 같은 실제 값이 거절되던 것, 런타임 계정에 `CREATE TEMPORARY TABLES`가 없을 때 서버 능력 probe가 서버를 "InnoDB 없음"으로 오판하던 것(이제 권한 이름을 댄다). 서버 능력 게이트 뒤 `mysql_adoption_flow`가 실패한 채 푸시했던 것도 바로잡았다([VERIFICATION](../product/VERIFICATION.md) 정정). 쓰기 조각도 같은 실제 스키마에서 확인했다: 댓글 생성·수정·soft 삭제, 좋아요·북마크·영상 요청 생성이 원본이 남기는 행과 같고(UUID v4, DB default 플래그, UTC 시각), 위조 owner·삭제된 영상·익명·남의 댓글은 거절되며 유일 인덱스는 `WIRE.ALREADY_EXISTS`다. 이 과정에서 표현식 기본값(`DEFAULT_GENERATED`)을 generated 컬럼으로 오인하던 결함도 고쳤다. 권한 조각도 확인했다(원본 `admin_required`·`super_admin_required` 규칙): 사용자 비활성화(SUPER_ADMIN은 누구나, ADMIN은 GENERAL만, 댓글 숨김 포함)·권한 변경·요청 거절·soft 삭제가 규칙대로 되고 거절된 호출은 행을 바꾸지 않는다. 비활성화한 회원이 계속 읽고 쓰던 결함이 비교 하네스에서 드러나 모든 정책에 `active(actor)`를 넣어 막았다. 표당 resource 하나라는 매핑 제약, 기본키가 외래 키인 1:1 표 2개(감정 요약·DNA)의 본인 행 정책, 승인(영상 생성), 로그인 자체(서버 확장 필요)는 남았다. 남은 차이: 좋아요·북마크 취소(물리 삭제 문법 없음), 제목 OR 채널 검색(filter가 AND만), 목록 total, JSON·낙관적 잠금 표 2개, 음수 범위 Int 문법, Mongo·Redis 의존 목록, 쓰기·인증(7.2.2·7.2.4). 자세한 내용은 [fixture README](../product/tests/fixtures/facereview-mysql/README.md). 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.4 완료 게이트 판정 (Codex 실행, 별도 검토 가능)

7.2.4 계약의 범위는 댓글 생성·수정·soft delete의 저장 의미와 권한, 좋아요·북마크 on/off/on의 행 상태, 본인 탈퇴 때 bounded 자식 정리와 Comment/User soft delete다. 새 격리 MySQL 8.4에서 `reference/writes.py`의 AIP `/apply` 검증이 0 실패했고, 원본 고정 revision `7948b7e721837fdb73c427cb54e12cad4ec2d5af`의 실제 댓글 SQLAlchemy 메서드와 제품 HTTP 결과도 생성·수정·타인 수정 거부·soft delete에서 대조됐다. `mysql_facereview_reads`는 1/1 통과했다. 이전 증거의 PG17 audit/상한 rollback, MySQL like/bookmark 및 탈퇴, audit·멱등·cache, 원본 like/bookmark toggle은 각각 기존 원시 로그와 manifest를 참조한다. helper의 고정 경로·포트·계정은 필수 환경 입력과 loopback/high-port/non-root/role 분리 검사로 바꿨다. 검증은 전체 Flask 앱·브라우저 route/UI parity를 뜻하지 않는다.

계약은 기존 AIP `/apply`·SDK `apply()`에 선언된 동작을 추가하도록 했고, source의 단일 toggle route/응답을 복제하라는 요구는 없다. create/remove를 유지하는 것이 원칙과 일치하므로 호환 REST endpoint는 추가하지 않는다. 브라우저 댓글 gate는 7.1.3.2.2, 전수 화면 상태는 7.1.3.2.3 및 SDK/UI 통합은 7.2.10에 남는다. MariaDB는 adapter 호환성 게이트인 7.2.1의 범위다. WBS 7.2.4는 코어 계약 기준 완료로 PMS를 갱신한다.

## 2026-10-18 WBS 8.7 릴리스 게이트 증거 묶음 (Claude 구현, 독립 검증 대기)

새 clone에서 전체 회귀, release 빌드·패키징, 묶음 smoke, 이전 릴리스(`4c27040`)의 schema 업그레이드와 두 롤백을 실행하는 `scripts/release-gate.sh`와 `product/tools/upgrade-drill.mjs`를 추가했다. 드릴이 실제 결함을 찾았다: 구버전 durable job schema를 새 binary가 거부했다(`b1b7d1b`). 후보는 main이 아니라 MySQL 트랙의 미완 커밋(fmt·clippy 실패)을 뺀 로컬 트리여서 main에서 게이트를 다시 실행해야 한다. 결과와 한계는 [릴리스 게이트](../product/RELEASE-GATE.md)를 따른다. 시작 가이드(`product/GETTING-STARTED.md`)와 개발용 토큰 helper를 더했다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.1.3 MySQL 작업 저장소·outbox 릴레이·MariaDB 게이트 (Claude 구현, 독립 검증 대기)

PostgreSQL job 저장소의 상태 기계를 MySQL 8.4로 옮겼다(`SKIP LOCKED` claim, DB 시계, 키 digest 유일 열, lease epoch fencing, backoff·deadline·cancel·redrive). 실 MySQL 8.4.11에서 40개 run을 8 worker가 정확히 한 번씩 claim했고, 후보 잠금·epoch 검사·epoch 증가를 각각 끄면 시험이 실패한다. outbox 릴레이는 claim·enqueue·영수증을 한 트랜잭션으로 해 4개 동시 릴레이가 30행을 한 번씩만 dispatch하고 실패 배치는 아무것도 남기지 않는다. 서버 능력 probe는 MySQL 8.4를 통과시키고 MariaDB 11.4.13을 LATERAL·`FOR SHARE OF` 부재로 이름을 대며 거부한다(SKIP LOCKED·JSON_ARRAYAGG·CHECK·InnoDB는 있다). MariaDB 11.4에서도 같은 job 저장소·릴레이 시험 3개가 통과한다(방언 둘: JSON 바인딩, `FOR UPDATE OF` 없음). HTTP `/jobs/*`와 job 실행(worker·effect journal)은 연결하지 않았다. 표와 한계는 [MySQL adapter 계약](porting/MYSQL-ADAPTER-CONTRACT.md)의 "작업 저장소" 절이다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.5 보존 정리와 S3 owner quota (Claude 구현, 독립 검증 대기)

S3 서명 업로드를 owner별 byte·개수 상한에 예약하는 장부와 만료 예약 정리(저장소 `HEAD` 확인 뒤 확정·해제·multipart abort), outbox 보존 기간 설정(`retention`)과 handler 없는 topic 정리, events 나이 기준 정리를 구현했다. 실제 TLS MinIO와 PostgreSQL 17에서 실행했고 AWS·GCS 실계정은 미실행이다. 기존 문서의 outbox 미정리 서술은 O-20 이전 상태여서 고쳤다. 실행 결과와 한계는 [제품 검증](../product/VERIFICATION.md)의 WBS 8.5를 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.11 8.9 후속 (Claude 구현, 독립 검증 대기)

spike-v3·v4 테스트의 사전 clippy 실패 3건을 정리해 세 spike의 `clippy --all-targets -D warnings`가 모두 통과한다. `Kind::Io`는 호출자의 COMMIT 호출에서 재현할 수 없어 접속 단계의 실제 Io 오류로 판정을 고정했다. 실행 결과는 [제품 검증](../product/VERIFICATION.md)의 WBS 8.11을 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.1.1 MySQL 읽기 경로 (Claude 구현, 독립 검증 대기)

PMS에서 review였던 7.2.1.1은 `aip-migrate` 드라이버와 DDL 펜스만 있었고 서비스 경로가 없었다. 이번에 `aip service adopt-external`·`principal`·`serve`와 `/read`·`/ready`를 실제 MySQL 8.4에 연결했다. 읽기 계획기에 `Dialect`를 넣었고 PostgreSQL 출력은 바이트 단위로 같다. 원시 카탈로그 행을 펜스 지문에 넣기 전에는 VARCHAR 길이·collation·생성식·인덱스 속성·CHECK·스토리지 엔진 변경 열두 가지를 지문이 보지 못했고, ENUM·DATETIME·DECIMAL 열이나 view가 있는 schema에서는 지문 계산 자체가 실패했다. 셋 다 실DB 시험으로 재현한 뒤 고쳤다. 트리거는 최소 권한 계정에 보이지 않아 감지하지 못하는 한계로 남기고 시험으로 고정했다. MariaDB 서빙·채택은 거부한다(LATERAL 부재). `/apply`·작업·파일·rate limit 등은 7.2.1.2·7.2.1.3 대상이며 명시 거절한다. 실행 명령과 결과, 의미 차이(collation 등)는 [MySQL adapter 계약](porting/MYSQL-ADAPTER-CONTRACT.md)의 "읽기 경로" 절을 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.1.2 MySQL 쓰기 경로 (Claude 구현, 독립 검증 대기)

`/apply`(전이·묶음·create)와 `/status`를 MySQL 8.4에 연결했다. PostgreSQL의 `RETURNING`·`xmin`·advisory lock이 없어 `FOR UPDATE` 판정, 매칭 행 수 확인, 엔진이 기록하는 touched 행, `aip_tx_locks` bucket 잠금, 같은 트랜잭션의 멱등 journal과 감사로 같은 계약을 다시 만들었다. 실 MySQL 8.4.11에서 원자성, 멱등 재생, 동시 용량·증가·같은 키·expect 경쟁, UUID 리소스, 부재 정책의 SERIALIZABLE 이중 예약 방지, COMMIT 전후 연결 단절 세 경우를 확인했고, 보호 장치를 하나씩 끄면 시험이 실패하는 것도 확인했다(판정 `FOR UPDATE`만은 `UPDATE`가 같은 보호를 해서 결과가 같다). `compose`·확장 쓰기·작업·outbox 전달은 명시 거절하고 7.2.1.3에 남긴다. 자세한 표와 한계는 [MySQL adapter 계약](porting/MYSQL-ADAPTER-CONTRACT.md)의 "쓰기 경로" 절이다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.9 후속: 나머지 COMMIT 경로와 spike-v6 사전 실패 (Claude 구현, 독립 검증 대기)

분류 규칙(COMMIT 전 단절은 `WIRE.DB_UNAVAILABLE`, 전송 뒤 단절·해석 불가 응답은 `WIRE.COMMIT_UNKNOWN`, 서버 거절은 기존 코드)을 `spike-v3-write`의 `apply`·`bundle`·`compose`, `spike-v4-worker`의 `invoke_write`, `write_extension.rs`, `durable.rs`에 적용했다. 세 spike는 `aip-service`·`aip-auth`가 path dependency로 쓰는 제품 경로라 문서화에 그치지 않고 고쳤다. 판정과 COMMIT 전송은 `spike-v3-write`의 공통 함수(`connection_failed`, `send_commit`, `before_commit_rejection`)로 묶었고 v4·v6 `/apply`가 같은 함수를 쓴다. 경로마다 장애 지점별 재현 테스트를 붙였고, 구현 전 red(v3 14개 중 12, v4 9개 중 8, v6 `write_extension` 10개 중 8, job의 COMMIT 손실 3개 모두 실패)와 구현 뒤 green, 분류를 되돌리는 변이 20종이 모두 실패하는 것을 확인했다. 변경 중 제품 동작이 바뀌는 곳은 managed effect journal의 COMMIT이다. 응답을 잃은 job은 확장 오류 대신 `WIRE.COMMIT_UNKNOWN`으로 끝나고, COMMIT 전에 연결을 잃은 job은 `WIRE.DB_UNAVAILABLE`로 재시도된다.

spike-v6 사전 실패는 셋 다 원인을 찾았다. `write_isolation`은 제품 결함이 아니라 `aip-test-support::database_url_with_option`이 URI 옵션의 공백을 `+`로 써서 PostgreSQL이 접속을 거부한 테스트 지원 결함이고(같은 함수를 쓰는 `aip-migrate`의 `migration_history` 2개도 main에서 실패했다), 고쳤다. pool 30ms 테스트는 첫 checkout의 연결 시간이 같은 기한을 쓰는 불안정 테스트라 연결을 미리 idle에 넣어 결정적으로 바꿨다. v12~v16의 `tsc` 부재는 새 worktree에 추적되지 않는 `node_modules`가 없는 환경 문제이며, 전역 설치 없이 저장소 경로(`npm ci --prefix spikes/spike-0-ts`)로 설치하자 5개가 통과했다. 실행 명령과 결과, 남은 미검증 항목은 [제품 검증](../product/VERIFICATION.md)의 "WBS 8.9 나머지 COMMIT 경로와 spike-v6 사전 실패"를 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.9 분류 보강 (Claude 구현, 독립 검증 대기)

8.9가 연결이 닫힌 경우만 `COMMIT_UNKNOWN`으로 분류하던 범위를 실제 재현 테스트로 넓혀 확인했다. COMMIT 전송 전에 연결을 잃으면 `WIRE.DB_UNAVAILABLE`(커밋 없음, 같은 키로 안전한 재시도), COMMIT 전송 뒤의 응답 손실·해석 불가·예상 밖 응답은 `WIRE.COMMIT_UNKNOWN`이다. 서버가 거절한 COMMIT(제약 위반, 직렬화 실패, 교착)은 확정 코드를 유지한다. `commit_unknown_wire` 11개(기존 1개와 신규 10개)가 red 5 failed에서 green 11 passed가 되었고 분류를 되돌리는 변이 4종이 모두 실패했다. spike-v6 clippy `-D warnings`와 `cargo fmt --all --check`는 통과했고, spike-v6 전체 스위트의 실패 7개 타깃은 수정 전 main에서도 같다. 실행 명령과 미검증 항목(`Kind::Io` 직접 재현 불가, 다른 COMMIT 경로 미실행 등)은 [제품 검증](../product/VERIFICATION.md)의 WBS 8.9 분류 보강을 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-17 WBS 8.x 통합 (Claude 구현·통합, GPT 독립 검증 미실시)

Codex 작업 세션이 취소된 7개 브랜치를 이어받아 검증하고 main에 통합했다. 병합된 것은 `codex/db-test-isolation-20261017`, `fix/wbs-8.1-serializable-concurrency`, `fix/wire-commit-unknown-20261017`(8.9), `codex/wbs-8-6-cli-lint-cleanup-20261017`(8.6), `feature/wbs-8.4-python-sdk`(8.4)이고, `feature/wbs-8.2-8.3-recovery-20261017`은 test tooling만 반영했다. `fix/otlp-status-code-standard`는 코드가 이미 main에 있어 병합하지 않았다. 브랜치별 실행 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 WBS 8.x 통합 회차를 따른다.

이 통합 Run에서 같은 환경(이 Run 전용 PostgreSQL, 컨테이너 IdP·Caddy)의 IdP·TLS smoke는 원래 설정(workers 포함)으로 17 passed, 0 failed였다. 이는 구현자 자체 실행이며 독립 검증은 아직 없다. 기존 독립 교차 검증의 FAILED(3/17) 기록은 그 환경의 사실로 유지한다. 8.2·8.3·8.4·8.6·8.9와 DB 가드는 모두 독립 검증 전이므로 완료로 표시하지 않고 "통합됨, 독립 검증 대기"로 둔다. DB 가드가 불완전해 공유 PostgreSQL(5432)에 의도치 않게 접속한 사고가 있었고 이 문서의 해당 절에 기록했다.

## 2026-10-17 포팅 진행 — UI 환경 갭과 MySQL adapter 경계

댓글 UI 7.1.3.2.2는 아직 실행하지 못했다. 설정된 Ttakkari browser가 loopback `23173`에 연결할 때 `ERR_CONNECTION_REFUSED`를 반환했고, 지정 browser preflight는 `status=blocked`, `browser_launched=false`와 Mach IPC 거부를 기록했다. 추가 브라우저 실행 경로로 우회하지 않았다. 외부 YouTube 모달이 원인일 수 있다는 설명은 미검증이며, 이를 AIP 제품 갭으로 분류하지 않는다. 다음 허용된 browser 실행에서만 iframe/API stub 또는 화면에 보이는 닫기 동작을 고정 절차로 추가한다. 상세 범위는 [브라우저 기준선](porting/BROWSER-BASELINE.md#2026-10-17-댓글-ui-실행-환경-갭)을 따른다.

AIP MySQL/MariaDB adapter의 실제 연결·catalog/read/write 검증은 여전히 미실행이다. `aip-migrate`의 PostgreSQL `tokio_postgres` 경로에 붙이지 않고, 허용된 기존 의존성으로 MySQL `INFORMATION_SCHEMA` 입력을 중립 manifest로 바꾸는 순수 함수·타입 계층을 추가했다. 합성 행 회귀는 PK/index/FK 순서·unsigned·enum·tinyint(1) 표시 폭·nullable/default/generated와 FK 대상·referential action을 보존하고, 중복/누락 metadata·미지원 action은 거부하며 varchar 길이·NULL default 모호성과 collation·생성식·on-update 등 표현 밖 의미를 진단한다. 원본 11개 RDB 모델의 선언 기준은 [FK 기준 manifest](porting/facereview-fk-manifest.json)다. 기존 댓글 PostgreSQL fixture는 Comment의 두 참조만 담으며 전체 모델 기준선은 아니다. 이는 DB 관측이나 MySQL adapter 지원이 아니다. MySQL/MariaDB URL의 `DB_ADAPTER_UNSUPPORTED` guard는 유지한다. 실제 driver·catalog query 연결과 MySQL 8.4 실 DB 검증, 이후 MariaDB 별도 실행 및 read/write는 남아 있으며 WBS 7.2.1 전체는 미완료다. 인증·권한 포팅은 그 뒤다. 코드 revision: `28bb463f4bb73f461ff813ec8da3f57ad726e165`.

## Infisical 후보 조사 및 Memoir PostgreSQL 작은 업무 적용 (2026-10-17)

공식 공개 저장소 `Infisical/infisical`의 고정판 `188cf58173019bf0fe59972ed1c7ef715bec979c`를 코드 검토했다. root 코드는 MIT Expat이고 `ee/`는 별도 상용 라이선스다. 프로젝트 편집 권한은 normalized Membership→MembershipRole 관계와 역할 평가를 거치며, 현재 DSL에 없는 관계 집합 권한 조회를 요구했다. 이를 한두 개 actor 참조로 축약하면 원본 권한 모델과 달라져 이 단위는 채택하지 않았다. Infisical 원본 서비스·운영 API·DB는 실행/접속하지 않았고 home-server secret/credential/settings도 읽지 않았다. 따라서 Infisical 적용·실DB 검증은 없다.

기록상 기존 PostgreSQL 후보 `port-trials/memoir-notes`에서 기존 실행 예제의 Note create/favorite/delete와 겹치지 않는 Folder rename을 선택했다. 고정 비교 commit `26396070917ee065b988b0de296baf54f6266322`의 `FolderService`, `FolderRepository`, entity와 request/controller를 기계적으로 대조한 범위 및 제한은 [시험 기록](../product/tests/fixtures/memoir-folder-rename/README.md)에 있다. 원본 worktree는 36개 사용자 변경이 있어 보존했고 HEAD의 해당 파일만 읽었다. 별도 전용 PostgreSQL DB에서 synthetic source-shaped row와 AIP resource를 채우고 서비스·설치형 TS SDK로 owner read/rename, unrelated actor의 빈 조회/거부, 거부 뒤 DB 무변경을 실행했다. 결과는 **코드 기준 적용 / 실DB AIP 검증**이다. Memoir 원본 서비스/API/DB는 실행하지 않아 원본 E2E 동등성은 주장하지 않는다.

실행 출력, 실제 저장값, 제한과 제외 범위는 [제품 검증](../product/VERIFICATION.md#memoir-folder-rename-실db-aip-검증-2026-10-17)을 따른다. 이 작은 시험은 Memoir 전체 포팅, Infisical 적용 또는 인증/권한 WBS 완료가 아니다. FaceReview WBS 7.1/7.2 gate, Infisical 전체 포팅 WBS 7.3, MySQL adapter gate는 그대로 미완료다.

## 페이스리뷰 원본 포팅 기준선 후속 (2026-10-17)

고정 원본 backend에서 허용된 로컬 Origin의 Socket.IO polling 연결, 빈 `watch_frame` 입력 거부, 연결 종료를 실제 확인했다.
이는 WBS 7.1.3.3.1의 부분 원본 기준선이며 AIP 포팅·실제 프레임·WebSocket·ML·worker/provider·통합 CI 완료를 뜻하지 않는다.
댓글 UI는 고정 YouTube 영상의 실제 제공자 오류 모달이 상호작용을 막아 열어 두었다. seed·player 동작을 바꿔 통과시키지 않았다.
API의 MySQL/Mongo/Redis 실제 서비스와 23 API case 기준선은 계속 재사용한다. 전체 범위와 다음 단계는 [포팅 README](porting/README.md)를 따른다.

> 현재 기준: [2026-10-09 백엔드 기능 포괄성 지침](sources/founder-backend-capability-directive-2026-10-09.md)과 기존 [통합 지침](sources/founder-integrated-directive-2026-10-03.md). 충돌하면 최신 원문의 범위·표식을 따른다.

## 외부 IdP·TLS reverse proxy 및 실제 broker·storage smoke 검증 (2026-10-17)

WBS 항목인 외부 IdP/TLS reverse proxy 운영 smoke 및 실제 Kafka/RabbitMQ/MinIO(S3 호환) 연동 smoke의 로컬 컨테이너 기반 검증 기록이다. CA 코드·문서 보완은 독립 검증 PASS다. IdP 전체 smoke는 독립 교차 검증에서 FAILED(3/17 실행, 나머지 14개 미실행)였고, 이후 통합 Run에서 원래 설정으로 17 passed였다. 후자는 자체 실행이라 WBS A는 독립 검증 전까지 미완료로 둔다.
- **외부 IdP 및 TLS reverse proxy**: Caddy TLS proxy 환경에서 자체 경량 RSA/JWKS/JWT fixture(`conformance/idp-proxy/idp/server.mjs`) 기반으로 검증했다. Keycloak·상용 IdP 연동 및 RFC 9068 전체 conformance 적합성은 미검증이다. 원 작업(`cbaa401`)에서는 17개 검사 통과를 보고했으나, 독립 교차 검증(`cbaa401`, 별도 DB)에서는 3개 assertion(health/discovery/JWKS) 통과 후 `aip service` 시작 단계에서 `SERVER_CONFIG`로 중단(exit 1)되었다. 후속 진단에서 실패 stage는 `worker_runtime_probe`로 확인되었다(macOS `isolation=auto` 환경의 `node --version` exit 71 / `permission_or_sandbox_denial`; nested Mac Run의 환경 제약이며 정확한 syscall/규칙 불명, production 일반화 금지). 프록시·CORS·request ID·JWT 검증 등 남은 14개 assertion은 여전히 미실행 상태다.
- **실제 Kafka·RabbitMQ 및 MinIO**: `scripts/smoke-brokers-storage.sh`의 세 cargo 명령군(`brokers_live` 2 passed, `aip-service` recovery 1 passed, `cloud_storage` 1 passed) 기반 총 4개 live 통합 검사(`real_kafka_confirm_and_import_round_trip`, `real_rabbitmq_confirm_and_import_round_trip`, `broker_worker_recovers_from_database_disconnect_without_losing_delivery_budget`, `minio_https_actual_object_and_multipart_contract`)를 통과했다(필터된 0개 타깃은 제외). MinIO 소스 빌드는 독립 검증 시 기존 로컬 이미지 재사용으로 미실행되었으며 공식 이미지 중단 단정은 제외했다.
- 상세 실행 결과 및 회귀 수정은 [제품 검증](../product/VERIFICATION.md)을 따른다. (작업 기준 부모 커밋: `4e8e2126a91bd15e21afd870fe5373c0ccba9adb`).

## 내부 metrics·OTLP exporter·durable job trace 연결 (2026-10-17)

WBS 항목인 내부 metrics 확장, OTLP/HTTP exporter, durable job run trace 연결을 구현했다.
1. 내부 metrics: HTTP 요청(`aip_requests_total`, `aip_request_duration_seconds`), operation(`aip_operations_total`, `aip_operation_duration_seconds`), durable job(`aip_jobs_total`, `aip_job_duration_seconds`)의 카운터 및 지연 histogram(5ms~30s)을 계측하고 AIP 오류 코드별 실패를 분류한다.
2. OTLP exporter: `OtlpConfig`로 설정하며 기본값은 off다. 비활성화 시 `Option` 검사만 수행되어 추가 오버헤드가 없다. 활성화 시 bounded mpsc queue(4096) 기반의 백그라운드 워커가 `/v1/traces`와 `/v1/metrics`로 OTLP JSON 규격 배치 전송을 수행한다.
3. Durable job trace 연결: HTTP 인그레스의 W3C `traceparent`에서 유입된 `trace_id`를 PostgreSQL `aip_jobs.trace_id`에 저장하여 재시도·attempt 간 보존한다. job 실행 시 해당 trace_id 컨텍스트를 복원하여 HTTP 요청 span과 durable job run span이 동일한 trace로 묶인다. 민감값(인증 토큰, 요청 body/query payload)은 span 속성에서 제외한다.
검증: mock in-process OTLP 수신기를 통한 통합 테스트(`tests/otlp.rs`), `aip-execution`, `aip-service`, `cargo fmt`, `cargo clippy`, `cargo deny` 전체 통과.

2026-10-17 후속 수정: commit `4e8e2126a91bd15e21afd870fe5373c0ccba9adb`가 `aip_jobs.trace_id`를 fresh DDL에만 추가해 이미 설치된 job schema를 자동 업그레이드하지 못했다. 기존 marker의 DDL digest가 현재 생성 DDL과 다르면 planner가 `derive()` 전 `WIRE.SCHEMA_MISMATCH`로 거부했고, 검증 전 Store enqueue/claim은 PostgreSQL `42703`을 냈다. 검증된 구형 digest 하나만 catalog marker와 함께 인식해 `aip service migrate` 계획에 nullable `trace_id` 추가를 넣고, 기존 migration transaction/journal 경로에서 적용하도록 수정했다. 임의 drift, 알 수 없는 digest, 이전 digest와 맞지 않는 실제 컬럼 구조는 계속 거부한다. 전용 PostgreSQL에서 `aip service init`과 실제 `aip service migrate` plan/apply/replay 명령을 거친 회귀는 기존 NULL job claim, 신규 enqueue/claim, idempotency replay, retry trace 보존, actor/principal/idempotency/facts/wire metadata 보존과 drift negative case를 확인했다. 독립 검증은 고정 source SHA `9bc78de3654c163e29f3cb369ec21ff496499e85`의 DB upgrade 범위에서 **PASS**이며 `verification_outcome=passed`로 기록한다. 이는 이 DB upgrade 범위의 판정이고, 전체 WBS·제품 출시·worker E2E 완료를 뜻하지 않는다. main 반영·production 배포 상태는 이 기록과 별도다.

OTLP 표준 status code 패치(`661024d6d23629b2eb276105eaba22144f0ff401`)의 코드와 테스트만 latest main에 통합했다. 독립 Collector 검증은 HTTP 200→`Ok`, 405→`Error`, job `SUCCEEDED`→`Ok`, `FAILED`/`CANCELLED`→`Error`, `RETRY`→`Unset`을 확인했다. 이 검증은 synthetic Lease 기반 emitter까지이며 worker/DB lifecycle 검증은 아니다. 원 branch의 `STATUS`/`VERIFICATION` 문서는 가져오지 않아 IdP 전체 FAILED(3/17, 14 미실행) 및 CA 독립 PASS 정정을 유지한다. exporter failure, parent flag 보존 등 다른 관측성 개선은 미완료 범위에 남긴다.

## resource 독립 실행 고도화 (2026-10-10)

후속 요청으로 [동기 operation](../product/EXECUTION.md)을 제품 V1/V2/V4/V5/V6와 운영 신원·배포·설치 SDK에 연결했다. 업무 resource 0개 모드와 기존 actor resource 모드를 구분하며, allow·입출력 범위·기한·ctx 거부·현재 principal 재검사를 유지한다. 다음 [durable Job 제안](backend-capability/durable-job-proposal.md)은 실행권 세대·권한 회수·취소/완료 경합·결과 ACL·schema 업그레이드 접점까지 구체화했지만 아직 구현하지 않았다. 현재 실행 근거는 [제품 검증](../product/VERIFICATION.md)을 따른다.

## 현재 포괄성 감사 (2026-10-09)

[백엔드 기능 감사 A~F](backend-capability/README.md)는 SQL 밖 기능을 포함한 105개 최소 항목·여섯 시나리오·Claude Code/Codex 상호 검토를 기록한다. 제품 지원과 별도 PoC를 구분했고, 업무 SQL 없는 Node/Python 계산·DB 불가·선언 deadline·6초 동기 실행을 실제 실험했다. 제품/PoC 재실행 범위와 미검증은 [검증 기록](backend-capability/validation.md)에 있다. 이 감사는 분석·제안이며 새 실행 모델의 대규모 변경을 수행하지 않았다. 최종 DSL·쓰기 모델·Id wire 등 기존 Open도 선점하지 않는다.

직전 작은 보완은 Ref 대상 identity·nullable 생성 후보이며 `ecf3369`에 따로 커밋했다. 후속 작은 허점과 비SQL 실행 모델의 우선순위는 [로드맵](backend-capability/roadmap-and-risks.md)을 따른다. 아래 제품/실험 수치는 각 시점의 이력이다.

## 현재 제품 작업 (2026-10-05)

사용자의 “운영인증, 일반마이그레이션, 제품통합 다 해” 지시로 범위를 확장했다. 기존 PoC는 보존하고 root CLI의 `aip service`와 `aip-auth`, `aip-migrate`, `aip-service`를 추가했다. caller 정의·서버 권한·기존 실행 엔진·하나의 SDK를 재사용한다.

운영 JWT 검증과 지속 actor 연결, 일반 DDL 계획·적용·배포 저널, 데이터와 멱등 결과를 보존하는 명시적 prototype 이관, 설치 SDK를 제품 경로에 연결했다. 재귀 제한은 source·token·AST·predicate 확장·path·SQL 생성·복제 payload에 함께 적용한다. 활성 서버의 구 정책 실행, 연결 재활성화 시 폐기 토큰 복구, Id wire 변경에 따른 멱등 namespace 변화도 회귀 검증 대상으로 고정한다.

2026-10-06 최종 제품 검증은 [제품 결과](../product/VERIFICATION.md)와 [배포 묶음](../product/dist/2026-10-06T11-19-25-091Z/README.md)을 따른다. 현재 실행 절차·검증 명령·미지원 범위는 [제품 서비스](../crates/aip-service/README.md), [마이그레이션](../crates/aip-migrate/README.md), [SDK](../product/SDK.md)를 따른다. 자체 fixture 기반 IdP·TLS proxy 및 실제 broker/storage smoke는 2026-10-17 로컬 컨테이너 기반으로 실행됐다. CA 코드·문서 보완은 독립 검증 PASS이나 IdP 전체 smoke는 독립 교차 검증에서 FAILED(worker runtime probe 이후 14개 assertion 미실행)였고, 통합 Run의 자체 실행은 17 passed다. 독립 재검증은 남아 있다. 다른 OS·무중단 대형 backfill·클라우드 상용 IdP/외부 서비스 직접 연동 및 RFC 9068 전체 conformance는 미검증이다. 최종 DSL·Id wire·쓰기 조합·라이선스 결정은 유지한다.

## 이전 단계: 2026-10-04 밤 작업 요약

설계 문서를 바탕으로 **작은 검증 실험 V1부터 V15까지 실행했다.** 본 `crates/`는 건드리지 않았고(통합 지침 §16·17), 실험은 `spikes/`의 독립 Rust crate에 있다. V1부터 V7까지는 기존 Codex 6.1-sol 검토 이력이 있고, V8·V9·V10·V11·V12·V13 후속 작업은 Codex 메인과 Luna high·Sol high가 검증했다. V14는 메인이 테스트·구현·회귀를 수행하고 Sol high가 독립 검토했다. Luna는 V14에서 사용 한도로 참여하지 못했다. V15에서는 다시 참여해 생성기 테스트·범위 대조를 맡았고, 메인이 구현·HTTP/DB·회귀를, Sol high가 설계·독립 검증을 맡았다. 찾은 문제는 재발 테스트로 고정했다.

1. **문법(V1)**: 독립 `.aip`, TS/Python 안 선언 블록, TS/Python 정형 데이터 5가지 작성 방식이 같은 의미(typed facts)로 바뀐다. 잘못된 기호·동적 문자열·주석 속 가짜 선언은 거부한다. 형식을 고르는 근거(작성 경험)는 아직 측정하지 않았다.
2. **호출자 읽기(V2)**: 화면이 고른 필드·관계·필터·정렬을 서버 정책 아래 매개변수 SQL 하나로 실행했다. 열린 필드는 요청만 바꾸면 되고, 닫힌 필드는 서버 정의 변경이 필요하다는 경계가 실제로 동작한다.
3. **쓰기(V3)**: 알림 읽음·모집 마감 같은 단순 쓰기는 표준 전이 하나로 원자성·동시성까지 처리됐다. 일괄 승인과 관리자 위임에서는 **서버 정의 전이(W2)가 순서까지 서버 안에 두고 업무 불변식을 지켰다.** 공개 동작 묶음(W0)과 호출자 조합(W1)은 커밋 시점 불변식·사후조건이 있어야 안전했고, W1은 업무마다 새 조합 개념(값 출처, 자기 행)이 필요했다. 어느 쪽을 표준으로 둘지는 아직 정하지 않는다.
4. **확장(V4)**: Node·Python 확장이 서버가 묶어 준 권한 안에서만 데이터를 읽고, 쓰기 확장은 서버 트랜잭션 안에서 공개 전이만 불러 실패·기한 초과 시 전부 되돌려졌다. 다만 **worker를 별도 프로세스로 띄우는 것만으로는 확장이 DB에 직접 붙는 것을 막지 못했다.** macOS에서는 네트워크 차단으로 막혔지만 운영 환경용 격리 방식은 아직 정하지 않았다. 후속 프로토타입 준비에서 공통 scalar·bounded stdout/stderr·Result 시작·읽기 Grant 정리를 연결했고 Rust23개와 실제 Node/Python 초과 쓰기 롤백·외부 취소·반복 호출, 음성 대조7종을 검증했다. 후속 READ 연결은 SafeNumber/DecimalString을 ctx와 입출력에 적용하고 생성 SDK·HTTP·명시적 MacNetDeny worker로8개 실제 흐름을 실행했다. 당시 V4 전체25개 실패0이며 후속 bootstrap과 WRITE 입력 전송 기한 보완은 프로토타입 최신 기록을 따른다. 후속 WRITE HTTP/생성 SDK는 같은 공개 전이·멱등 저장소·원자적 결과 저장을 사용하며 [prototype 최신 결과](../prototype/README.md#write-공식-확장의-실행-결과)를 따른다. 운영 격리는 남는다. 외부 효과(알림)는 outbox로 "커밋된 것만 전달 시도, 실패 재시도 뒤 격리, 멱등 키로 중복 효과 억제"를 확인했다.
5. **프론트 SDK(V5)**: 화면이 고른 필드에서 결과 타입이 추론되고 계약 밖 요청은 컴파일 오류가 된다. 캐시는 정책이 읽는 권한 테이블을 포함한 읽기 의존 resource로 무효화하며, 같은 연결의 쓰기·계정 전환·결과 미확정·늦은 응답을 처리한다. 다른 연결의 변경·권한 회수는 V9의 수명 만료 뒤 재조회까지 지연될 수 있다.
6. **전송(V6)**: 최소 HTTP 서버와 TS 클라이언트를 연결했다. 쓰기 응답이 유실돼도 같은 멱등 키로 다시 보내 결과를 확정하고, 두 번 실행되지 않는다. 인증은 V8에서 서버 서명 토큰으로 붙였다(위조·변조·만료·다른 키 거부, 실패 토큰은 익명으로 낮추지 않음).
7. **마이그레이션(V7)**: 두 계약 버전의 변경을 안전·호출자 영향·보안 검토·파괴적·데이터 위반으로 나누고, 행 정책 변경은 실제 회원별로 보이는 행을 측정해 검토 근거로 붙인다(측정만으로 안전을 확정하지 않음).
8. **수명·복구(V9)**: 캐시는 검증한 세션 수명과 실험용 신선도 상한 안에서만 재사용한다. 시간 의존 조회는 현재 시각으로 실행하고 미캐시한다. 같은 멱등 키의 반복·동시 호출, 인증 만료 뒤 미확정 쓰기 보존, 잘못된 성공 응답, 헤더 크기 상한을 반례로 검증했다. [실행 결과·한계](alignment/V9-cache-lifetime-results.md).
9. **세션 갱신(V10)**: 첫 read/apply 전에 서버 principal을 확인한다. 같은 actor의 새 토큰으로 pending을 자동 복구하고 다른 actor 교체는 원래 정보를 유지한 채 거부한다. 캐시와 진행 중 요청을 인증 세대로 분리하고 signed -1과 익명 멱등 scope를 분리했다. 실제 토큰 만료·승인 응답 유실 뒤 replay와 DB 단일 효과를 확인했다. [실행 결과·한계](alignment/V10-session-recovery-results.md).
10. **선택적 검사(V11)**: 개발 조언 off/on을 실행 계약과 분리했다. docs 없음·변경·소속 이동이 실행 facts·생성 TS·DB 권한을 바꾸지 않는지 대조했고, 조언이 있어도 정상 실행을 막지 않는다. raw 요청의 타입·권한·비용 검사는 필수다. [실행 결과·한계](alignment/V11-optional-checks-results.md).

11. **계약 타입 연결(V12)**: 앱별 생성 binding만 전달하면 실제 HTTP 결과의 선택형 타입을 얻는다. 계약 식별값과 성공 envelope를 캐시 저장 전에 검사하고, 구세대 오류·미확정 쓰기 경계를 유지한다. typed facade의 캐시 주입 우회는 닫았다. 큰 Id 표현·typed apply·전체 행 decoder는 남는다. [실행 결과·한계](alignment/V12-typed-transport-results.md).

12. **큰 Id 왕복(V13)**: Legacy 반올림으로 이웃 행을 변경하는 실제 DB 반례를 재현했다. 숫자 안전범위 제한과 십진 문자열 후보는 서버 공통 경계에서 처리하며, 계약 불일치는 첫 쓰기도 DB 전에 거부한다. 공개 Id 표현은 아직 선택하지 않았다. [실행 결과·한계](alignment/V13-id-boundary-results.md).

13. **공개 쓰기 타입(V14)**: 같은 binding에서 공개 전이 입력과 readonly 결과가 추론된다. 두 Id 후보·읽기 없는 쓰기·응답 유실/손상 복구·실제 결과 동결을 검증했다. 공개 쓰기 변경도 지문에 포함하며 서버 최종 검사는 유지한다. V14 당시 미지원 scalar where는 후속 V15에서 연결했다. 조합/worker 타입·최종 API는 남는다. [실행 결과·한계](alignment/V14-typed-apply-results.md).

14. **공개 필터 실행(V15)**: Url·Enum·Time read/where 공통 검사와 Text.prefix를 실제 PG/HTTP에서 대조했다. 날짜·offset·소수초 반올림·NUL 선거부·타입/지문·권한을 확인했다. [실행 결과·한계](alignment/V15-filter-value-results.md).

15. **실행 프로토타입 연결**: [check/gen/init/serve CLI·단일 SDK](../prototype/README.md)를 기존 엔진으로 연결했다. CLI22개와 데이터 보존·전체 DDL 롤백·DB 설정·종료·세 음성 대조를 검증했다. safe/decimal 각각 실제 생성 SDK→서버→공개 정의 변경→재생성·재기동에서 구 binding 첫 쓰기 거부와 새 binding 실행, 기존 데이터·멱등 결과 보존을 확인했다. Sol은 소스 SDK decimal 흐름을 독립 재실행했다. 로컬 private SDK tarball은 새 소비자의 offline 설치·일반 JS 실행·strict TS 검사와 출력 보호를4개 테스트로 검증했고, source/package × decimal/safe4개 실제 서버 흐름을 실행했다. 새 Luna가 패키지4개·설치 SDK 두 wire를 독립 재실행했다. 공식 READ 확장도 단일 생성 binding/SDK에서 source/package × safe/decimal × Node/Python8개 조합을 실행했다. 당시 SDK 경계9개·Rust88개와 실제 child 수명/동시4개 상한·실행 guard8종 음성 대조를 검증했다. 후속 HTTP Origin/파싱5초/연결64개·캐시 전 표준 read 검사·관계 대상 집계·내장 worker를 보완했다. WRITE까지 생성 계약·HTTP·SDK에 연결해 실제8개 source/package/wire/lang 조합·동시 같은 key 단일 실행·pre-commit rollback·실제 COMMIT_UNKNOWN 뒤 재생을 확인했다. 정본 Event.confirm과 bundle의 두 언어 구현을 제공하며 CLI27개·SDK/패키지37개·builder5개·Chrome8개·재배치 bundle8개를 검증했다. CPU loop의 동시4개/5번째 거부·기한·child/row lock 반환과 상한/key lock 음성 대조도 확인했다. DB connect/clock 각각5초·owned driver·SQL operation deadline·init/serve SQL5초와 실제 catalog 구조 대조까지 보완했다. 실제 COMMIT 응답 손실과 다음 retry BEGIN 정지에도 pending/cache unknown을 보존하고 같은 key 재생으로 복구했다. 실행 중 만료된 세션이 이미 커밋한 WRITE의 성공을 실패로 바꾸는 반례도 Node/Python·두 wire에서 수정·검증했다. 정의 숫자 overflow·중첩 abort·캐시의 WRITE 완료 간격·닫힌 stdout/stderr도 보완했다. 최신 Rust175개(V1 포함)·SDK47개·builder5개·WRITE8개·Chrome8개·재배치 bundle8개는 실패0이다. [DB 후속 결과](../prototype/README.md#정의-입력캐시-완료cli-출력-후속-결과)의 marker 호환·검증 범위를 따른다. 최신 명령·범위는 prototype 후속 결과를 따른다. 이 항목은 제품 통합 이전 host의 private 실험 이력이다. 이후 운영 인증·일반 migration·제품 조립의 현재 결과는 위 현재 제품 작업을 따른다. 최종 WRITE API/조합은 남는다.

아리아리 원본과 다르게 동작한 점(코드 읽기 + 실험 비교, 원본은 실행 확인 안 함): 일괄 승인에서 없는 지원 id를 조용히 빼고 나머지만 승인하는 것으로 보임, 권한보다 동아리 검사가 먼저, 회원 중복을 막는 DB 제약 없음, 자기 자신에게 관리자 위임 시 관리자가 사라질 것으로 보임.

**지금 답이 필요한 질문은 없다.** 아래는 기술 비교가 더 진행되면 창시자에게 갈 수 있는 선택이다(아직 질문 아님).
- 권한 때문에 숨긴 필드를 null로 줄지, 빼고 줄지, 요청을 거부할지(V2-R1)
- 쓰기 결과가 불확실할 때(커밋 중 시간 초과) 호출자 경험: 재시도·상태 조회·멱등 키(V3-R6)
- 초기 표준 쓰기에 어느 정도의 호출자 조합 자유를 줄지(V3 §6·§7)
- 확장의 외부 네트워크 허용 범위와 운영 환경 격리 수준(V4 §3)
- Id 공개 표현: 큰 정수 Id를 문자열로 주고받을지, 숫자로 두고 범위를 제한할지(V5 §7 R5-05)
- 화면 데이터의 신선도: 다른 사용자의 변경을 얼마나 빨리 반영할지, 마감 시각처럼 시간이 지나면 바뀌는 조회를 얼마나 낡게 허용할지(V5 §6·§7)

## 현재 상태

요청된 A~E 설계 산출물을 작성했다. 창시자 목적과 기술 상세 승인은 분리했고, 기존 결정 문서의 확인 대기를 답변/검증 상태로 갱신했다. 코드·신규 문법 실행 검증은 이 문서 작업에 포함하지 않았다.

| 읽을 곳 | 현재 내용 |
|---|---|
| [A](alignment/A-founder-intent-matrix.md) | 최초 7원칙·각 FOUNDER 구절의 실제 code/doc 대응·누락 |
| [B](alignment/B-decision-reclassification.md) | 25개 질문·Q0~Q9·14개 의제 재분류, DD별 목적/상세 상태 정본 |
| [C](alignment/C-syntax-proposal.md) | 서버 A/E/H·프론트 read/apply·조합/확장/metadata 후보 |
| [D](alignment/D-development-experience.md) | 아리아리 실제 소스와 10개 변경 시나리오 비교. 절감률/성능 수치 미측정 |
| [E](alignment/E-technical-risks.md) | RK-01~10 반례·초기 실험 순서·V1부터 V15까지 실행 범위와 남은 항목 |
| [독립 대조](reviews/INTEGRATED-codex-luna-r1.md) | Luna high의 의미 누락 3건과 메인 확인/문법 반영 |
| [후속 판단](90-open-questions.md) | 비교 후 남을 출시/DX/호환/라이선스 선택. 이미 답한 25개를 재질문하지 않음 |

확인된 기준: 호출자 읽기·단순 쓰기·풍부한 표준 동작, Rust 자체 포트 서버+공식 프론트 SDK, JS/TS·Python 공식 확장, 선택적 구조화 설명, 선택적 개발 검사와 필수 실행 안전 검사 분리, 오픈소스·외부 기여.

미정 또는 검증 대상: 독립 파일 여부·구체 문법/Typed IR·안전한 쓰기 조합·초기 언어/프론트/서드파티 범위·비용 제어·worker/ABI·DB·호환 기간·라이선스. DIRECTION의 운영 입장·승인·롤백·동기화·오류 정책을 확정으로 승격하지 않았다.

이번 협업: Claude Code는 토큰 소진으로 미참여. Codex 메인이 설계·구현·판정하고 Luna high가 조사·인증 검증·테스트를, Sol high가 설계·동시성·수명 경계 비평을 맡았다. 기존 Claude/Codex 합의는 이전 지침 검토 이력이다.

문서 구조 검증·미실행 범위는 [V0 검증 결과](alignment/V0-validation-results.md)에 남긴다.

### 2026-10-04 밤 자율 작업 (spike 실험, 본 crates 미변경)

| 실험 | 결과 | 독립 검토 |
|---|---|---|
| [V1 의미 fixture](alignment/V1-semantic-fixture-results.md) | `spikes/spike-v1-fixture/`. A·E-TS·E-Py·H-TS·H-Py 5형식이 같은 typed facts. 음성 대조 49건 + r1·r2 반례 + 손으로 쓴 기대 facts 대조. 최신 전체 재실행 17 passed | Codex 6.1-sol r1 "부분 성립"(P1 8건), r2 3건 → 모두 반영·재발 테스트 고정 |
| [V2 호출자 읽기 slice](alignment/V2-caller-read-results.md) | `spikes/spike-v2-read/`. facts만으로 매개변수 SQL 생성, 로컬 PG에서 행·필드·관계·집계 정책, 열린/닫힌 capability, filter/sort 별도 허용, depth/rows/cost/deadline·출력 상한 거부 확인. 5 passed | Codex r2b(실제 DB 재현) 12건 중 읽기 쪽 반영 |
| [V3 표준 쓰기](alignment/V3-standard-write-results.md) | `spikes/spike-v3-write/`. V3-1 알림 읽음·모집 마감(대상·상한·원자성·동시성·권한 의존 행 공유 잠금·기한). V3-2 일괄 승인을 W2(서버 정의 전이)·W0(공개 동작 묶음)·W1(서버 선언 범위 조합)로 같은 반례 표에 비교. 5 passed. V3-3 관리자 위임: 교대형 불변식에 커밋 시점 제약 필요, W0·W1은 순서 민감. W1은 compose에 selfRow를 더해 표현했고 반례 결과가 W0과 같음 | V3-1 Codex r2b, V3-2 Codex r3 반영, V3-3 r4b 발견 없음, W1 selfRow r6 반영 |
| [V4 공식 확장](alignment/V4-official-extension-results.md) | `spikes/spike-v4-worker/`. Rust 서버 + Node/Python worker. ctx 토큰(actor·허용 집계·입력 고정·기한), 출력 검사, 기한, 토큰 폐기, worker 장애. **발견: 환경 변수를 지워도 worker가 로컬 DB에 직접 연결 가능(격리 없음)**. V4-3 쓰기 확장: 서버 트랜잭션 안 공개 전이만, 실패·오류 삼킴·기한·늦은 ctx 모두 rollback, 커밋 결과 미확정 구분 | Codex r4(필터 중단)·r4b·r6 반영 |
| [V5 SDK](alignment/V5-sdk-results.md) | `spikes/spike-v5-sdk/`. facts에서 TS 계약 타입 생성, 화면 select에서 결과 행 타입 추론(관계 포함), 닫힌 필드·허용 밖 filter/sort·값 타입·루트 조회 불가 8가지를 컴파일 오류로. tsc 음성 대조 포함. V5-2 읽기 deps 건전성, V5-3 캐시 + V9 수명(node 10). V12 공개 읽기 식별값·V13 wire·V14 공개 쓰기 projection 대조 포함 최신 Rust 17 passed | V5-1·2 Codex r4b, V5-3 Codex r5 반영 |
| [V6 전송](alignment/V6-transport-results.md) | `spikes/spike-v6-transport/`. Rust 최소 HTTP 서버 + TS 클라이언트(V5 캐시). 쓰기 태그로 캐시 무효화, 응답 유실 시 같은 키 재요청으로 확정, 재실행 없음, 확정 전 캐시 저장 보류. node e2e 7 | Codex r7 반영 |
| [V7 마이그레이션](alignment/V7-migration-results.md) | `spikes/spike-v7-migrate/`. facts 두 버전 비교 → 변경 9종 분류, 행 정책 확대/축소를 회원별 보이는 행으로 측정, 불변식·enum·필수 필드·범위는 기존 데이터 위반 수를 적용 전에 계산. 분류하지 못한 차이는 모두 검토 | Codex r8 반영 |
| [V8 인증](alignment/V8-auth-results.md) | V6에 서버 서명 세션 토큰(HMAC, 기동 시 무작위 키, 만료). 시험용 actor 헤더 제거. 8사례 + 단위. V9에서 입력 크기와 캐시 만료 경계 추가 | Luna high 단위·서버 검증, 메인 반례·보강 |
| [V9 수명·복구](alignment/V9-cache-lifetime-results.md) | V5/V6 재사용. 당시 캐시·복구 Node 17, 실제 서버/DB Node 4. 세션 만료·시간 조회·다른 연결의 변경/권한 회수·같은 키 동시 호출·미확정 복구·입력 상한 | Sol high 설계·독립 비평, Luna high 인증 대조·테스트 작성 |
| [V10 세션 복구](alignment/V10-session-recovery-results.md) | 서버 `/session`, SDK `replaceSession`, 같은 actor pending 복구, 인증 세대 경합, signed -1/익명 분리. 최신 캐시/복구 단위 Node 27, 새 HTTP/DB Node 3 | Sol high 설계·최초 거부 반례, Luna high 테스트·문서 대조, 메인 실제 만료·회귀 검증 |
| [V11 선택적 검사](alignment/V11-optional-checks-results.md) | 기존 V1/V2/V5 재사용. 옵션/metadata/생성 TS/필수 거부 단위 8개·PG 24조합 1개. 고장 주입 3종 검출 | Luna high 단위 실패 대조, Sol high 설계·구현 리뷰, 메인 구현·PG·고장 주입 검증 |
| [V12 계약 타입 연결](alignment/V12-typed-transport-results.md) | V5 generic core·V6 typed facade. 생성 계약 식별값·실제 HTTP/PG·tsc 정상/음성·Node 11개. binding 누락/캐시 주입 반례 수정, 고장 주입 3종 검출 | Luna high 테스트·V5/V11 회귀, Sol high 설계·독립 우회 재현·수정 후 재검증 |
| [V13 큰 Id 왕복](alignment/V13-id-boundary-results.md) | Legacy 잘못된 행 재현·SafeNumber/DecimalString PG/HTTP·첫 apply 계약 거부·모드별 멱등·고장 주입 6종. V2/V3/V5/V6 전체 Rust 12·5·9·10개 실패 0 | Luna high 단위/회귀·범위 대조, Sol high 첫 apply 비평·guard Node 3개 독립 재검증 |
| [V14 공개 쓰기 타입](alignment/V14-typed-apply-results.md) | V5/V6 후보 binding·action/target/where 타입·두 Id mode·PG/HTTP Node 14개씩·형식 오류 pending·실제 결과 동결·고장 주입 6종. 전체 Rust V5 13·V6 11·V11 9개 실패 0 | 메인 테스트/구현/회귀, Sol high 초과 키·결과 변조 재현과 수정 후 독립 tsc/Node 재검증 |
| [V15 공개 필터 실행](alignment/V15-filter-value-results.md) | Url/Enum/Time read·where와 리터럴 prefix, 필수 값 검사·PG 반올림·NUL·타입/지문·권한·7종 고장 주입. 전체 Rust V2 19·V3 5·V5 17·V6 12·V11 9개 실패0 | Luna high 생성기 테스트·범위 검토, Sol high Time/PG·독립 검사, 메인 구현·HTTP/DB·전체 회귀 |

V3-2 관찰(검증 필요): 세 후보 모두 공통 반례(다른 동아리·권한 없음·누락 id·이미 회원·거절됨·동시 승인)를 같은 안전한 결과로 막았다. W0은 커밋 검사 없이는 지원하지 않은 회원 생성이 가능했고, W1은 허용 단계·값 출처·생성 allow(역할 제한 포함)·사후조건 두 개를 선언해야 W2와 같은 업무를 지켰다. 이 fixture의 선언 구성에서는 W1이 W2보다 서버 정의를 줄이지 못했다(11줄 vs 8줄). W1이 항상 불리하다는 근거는 아니며, 관리자 위임은 이후 V3-3에서 실행했다. 순환 전이와 다른 업무에서 조합의 장점이 유지되는지는 남은 비교다.

V13은 큰 Id의 두 후보를 실제 왕복으로 비교했다. 적용은 V6 HTTP read/apply와 후속 공식 READ worker이며 bundle/compose/WRITE worker·최종 표현 선택·일반 Int 정밀도는 남는다. V14는 direct 공개 전이의 생성 타입·응답 검사·쓰기 식별값을 실행했다. 후속 [V15 공개 필터 값 정합](alignment/V15-filter-value-results.md)은 Url/Enum/Time read/where·Text.prefix를 연결했다. [프로토타입](../prototype/README.md)은 초기화→생성 SDK→서버 호출→공개 정의 변경·재기동을 한 실행 흐름으로 검증했다. SDK tarball의 새 소비자 설치와 현재 macOS host의 재배치 bundle은 검증했으나 전체 제품 clean install, create/compose 값 타입·WRITE 확장 통합 등의 공백은 남는다. prototype 표준 read의 공개 선택 행·관계 decoder는 캐시 저장 전에 연결했다. 모든 값/API의 최종 domain을 확정한 것은 아니다. 공식 READ는 생성 타입/descriptor·서명 세션·필수 지문·worker 값/ctx 검사로 연결했다. 시간 의존 조회의 명시적 유효 시각, 다른 연결 변경 통보·재연결 복구, 운영 환경 worker 격리 비교도 남는다. V10은 토큰 교체 후 복구만 검증했으며 로그인·refresh-token 제공자와 브라우저 재시작 뒤 영속 복구는 남는다. V11은 직접 참조 부재 조언 한 가지와 resource docs만 검증했으며 제품 검사 전체·설명 소속 전체는 남는다. 기존 독립 검토·쓰기 확장·SDK 작업은 위 결과 문서를 따른다.

이 실험들은 문법·planner·쓰기 모델 결정이 아니다. spike가 임의로 정한 규칙(V1-R*, V2-R*, V3-R*, V4-R*)은 각 결과 문서에 기술 후보로 적었다.

## 이전 진행 현황과 로그

아래 합의/질문 수·사용량·자료 요청·아리아리 결함은 당시 작업 이력이다. 현재 대기 상태·실행 검증 결과로 재사용하지 않는다.

<details>
<summary>통합 지침 이전 진행 기록 보기</summary>

> 창시자가 한 번에 확인하는 문서. 위에서부터 읽으면 된다. 마지막 갱신 시각은 각 절 머리에 적는다.
> 작업 방식: 결정 문서마다 Claude 초안 → Codex(gpt-5.6-sol) 비판 → Claude 반영 → 합의까지 반복. Codex가 연결되지 않으면 Claude Sonnet 서브에이전트로 대체한다.

## 0. 저녁에 먼저 읽을 것 (2026-10-03 19시 기준)

1. **결정 문서 14개를 만들었고, 14개 모두 에이전트 간 합의에 도달했다.** 문서 간 교차 정합성 검토도 1회 거쳤다(P0 0, P1 14건 반영). 합의는 Claude와 비판자(Codex gpt-5.6-sol 또는 Claude Sonnet) 사이의 합의일 뿐이고, 결정 상태는 모두 미결정이다. 창시자 답이 있어야 확정된다.
2. **답해 주실 질문은 25개.** `90-open-questions.md` 맨 위 표에 순서대로 있다. 핵심은 앞의 1~7번(읽기·쓰기 모델)이고, 8~13번(작성 방식, 자연어 문법화)은 예시를 보고 고르게 돼 있다.
3. **원본 자료 요청 3건**: "최초 7가지 원칙" 원문, A형·B형 문법 예시, "자연어의 문법화" 설명(예시 문장 하나면 충분).
4. **아리아리 코드에서 찾은 결함(별도 확인 권장, 모두 코드 읽기 추정, 실행 확인 안 함)**:
   - 서비스 공지·FAQ 등록·수정·삭제 8개 API에 역할 검사가 없음(`SystemNoticeService`, `SystemFaqService`에 "검증 로직 추가 필요" 주석). 설명은 "운영 관리자만"
   - 진행 중 모집 조회·모집 존재 여부 조회에 학교 가시성 검사 없음
   - 조회수 일별 집계 키에 현재 조회수 값이 들어가 "최근 14일 조회수"가 의도대로 계산되지 않을 가능성(`ViewsManager.java` 87행)
   - 지원자 일괄 승인에서 권한 검사보다 동아리 검사가 먼저라 응답 차이로 id 존재를 추측할 수 있음
   - 오류 응답: 커스텀 예외는 HTTP 400 고정인데 본문 code는 403·404 등, DB 오류는 SQL 오류 메시지를 클라이언트에 그대로 돌려줌(`ExceptionControllerAdvice.java`)
5. 토론 기록은 `reviews/`에 문서별·라운드별로 남아 있다. 각 결정 문서 변경 이력에 무엇이 왜 바뀌었는지 적혀 있다.

## 1. 한눈에 보기

| 문서 | 주제 | 상태 | 창시자 질문 |
|---|---|---|---|
| DD-01 | 화면이 새 데이터를 원할 때 서버에 조회를 추가해야 하나(읽기 조합) | 에이전트 합의(3라운드) | F1~F4 |
| DD-02 | 쓰기에서 호출자는 무엇을 표현하고 서버는 무엇을 쥐나 | 에이전트 합의(3라운드) | F1·F2 |
| DD-03 | 조합 읽기를 운영 서버가 언제부터 받아 주나 | 에이전트 합의(3라운드) | F1 |
| DD-04 | 서버 쪽 정의를 무엇으로 쓰나(`.aip`·TS·Python) | 에이전트 합의(Codex 3라운드) | F1~F3 |
| DD-05 | "자연어의 문법화"가 무엇인가 | 에이전트 합의(Sonnet 3라운드) | F1~F3 (예시 먼저 → 원하는 효과 → 실행 영향) |
| DD-06 | Rust 엔진과 JS/Python 앱의 관계(독립 서버 / 임베드 / 설치 도구가 서버 자동 기동) | 에이전트 합의(Sonnet 2라운드) | F1~F3 (배포 환경 → 엔진 위치 → 확장 작성자) |
| DD-08 | 호출자 요청을 어떤 형태로 보내나 | 에이전트 합의(Sonnet 2라운드). 추천: JSON 트리 + 값 분리 | 없음(기술 제안) |
| DD-09 | 상세를 열면 올라가는 익명 통계(조회수) | 에이전트 합의(Sonnet 2라운드). 열람 지점 선언 방식은 DD-01과 함께 미결정 | F1 |
| DD-10 | "내가 북마크했나" 같은 보는 사람 기준 값 | 에이전트 합의(Sonnet 3라운드) | 없음(DD-01 F1·DD-04에 종속) |
| DD-11 | 쓰기 뒤 화면의 어떤 데이터를 다시 불러올지(프론트 캐시 무효화) | 에이전트 합의(Sonnet 3라운드). 추천: 읽기·쓰기 양쪽 태그 | F1(남의 변경이 얼마나 빨리 보여야 하나) |
| DD-12 | 서버 정의가 바뀔 때 무엇을 사람이 확인한 뒤 반영하나(스키마 진화·롤백) | 에이전트 합의(Sonnet 3라운드) | F1~F3 |
| DD-13 | 프론트 SDK가 무엇을 맡나 | 에이전트 합의(Sonnet 3라운드) | F1(라이브러리 범위: 데이터만 / UI 조각까지), F2(첫 출시 지원 프레임워크) |
| DD-14 | 응답과 오류의 모양 | 에이전트 합의(Sonnet 2라운드) | F1(권한 부족을 얼마나 알릴지) |
| DD-07 | 여러 건을 한 번에 처리하다 일부가 실패하면 | 에이전트 합의(Sonnet 2라운드). 추천: 전부 취소 + 안 되는 항목 한 번에 모두 알림 | F1 |

상태 어휘: 확정(창시자 승인) / 잠정 합의 / 검증 필요 / 미결정. "에이전트 합의"는 Claude와 Codex 사이의 합의일 뿐 결정 상태는 모두 **미결정**이다.

## 2. 쌓인 질문 (답하기 쉬운 순서)

`90-open-questions.md` 맨 위 표가 정본이다. 새 질문은 거기에 추가하고 여기에는 요약만 둔다.

- 총 25개: DD-01 F1~F4, DD-02 F1·F2, DD-03 F1, DD-04 F1~F3, DD-05 F1~F3, DD-06 F1~F3, DD-07 F1, DD-09 F1, DD-11 F1, DD-12 F1~F3, DD-13 F1·F2, DD-14 F1. `90-open-questions.md` 맨 위 표에 순서대로
- 원본 자료 요청: Q0 "최초 7가지 원칙" 원문, Q3 A형·B형 문법 예시, Q5 "자연어의 문법화" 설명

## 3. 진행 로그

- 2026-10-03 오후: DD-01~03 각각 Codex와 3라운드 토론, 에이전트 합의. 질문 7개 정리
- 2026-10-03 오후: 창시자 외출. 자율 진행 시작(다음 결정 문서 DD-04부터)
- 15:30 DD-04·05·07 초안. DD-04는 Codex 1라운드. Codex 사용량 5시간 한도 31% 남음(19:27 리셋) → DD-07부터 Sonnet 서브에이전트 병행
- DD-05 실험 E1: 아리아리 API 설명 문장 중 권한 규칙 36개, 그중 12개 코드 대조 → 불일치 2, 예외 누락 1, 강제 불완전 1, "관리자" 용어 모호
- DD-04 Codex 1라운드: 초안 추천(TS로 쓰고 `.aip`는 읽기 표기)이 사실상 고유 문법을 쓰는 형식에서 없애는 것이라는 지적, 예시가 같은 내용이 아니었음, TS 정의 실행 시 보안 경계 누락 → v2에서 추천 철회, 같은 9개 항목을 담은 예시 4개, "TS/Python 프로젝트 안 AIP 선언 블록"을 1순위 검증 후보로. Codex 사용량 24% 남음
- DD-07 Sonnet 1라운드: 사전 점검(U3)은 점검과 실행 사이 상태 변화로 같은 실패가 남고, 점검 자체가 정보 노출 통로가 된다는 지적 + 아리아리 코드의 권한 검사 앞 동아리 검사 오라클 발견 → v2 추천 변경
- DD-04 Codex 2라운드: 예시에 필드·간선 정책 누락, 확장 참조 규칙 충돌, F1·F2 모순 가능 → v3. Codex 사용량 18% 남음, 이후 문서는 Sonnet으로
- DD-01 실험 E1: 아리아리 모집 GET 11개 모두 수정 대안 2로 표현 가능. 새 요구 2개 발견(읽기에 딸린 조회수 증가, "내 동아리인가" 같은 행위자 기준 값) → DD-09·DD-10 예정. 원본 코드에서 학교 가시성 검사가 빠진 조회 2곳 발견(존재 여부, 진행 중 단건. 코드 읽기 추정)
- **중요 발견(DD-05 Sonnet 대조, 코드 확인)**: 아리아리 서비스 공지·FAQ 8개 API가 설명은 "운영 관리자만"인데 서비스 코드에 역할 검사가 없고 `// 검증 로직 추가 필요` 주석만 있음. 로그인 회원이면 서비스 공지 등록 가능으로 보임(실행 확인 안 함, 프론트·게이트웨이 보호 여부 확인 필요). 권한 설명 36개 중 불일치 13개. 아리아리 운영 중이라면 별도로 확인 권장
- DD-09 Sonnet 1라운드: 아리아리 조회수 일별 키에 현재 조회수 값이 들어가 14일 창이 의도대로 동작하지 않을 가능성(`ViewsManager.java` 87행, 코드 확인), PoC는 창 집계 미구현·중복 제거 키가 호출자 헤더 → v2(선언한 상세 형태만 집계, 서버 파생 키, 식별 값 해시)
- Codex가 DD-11 실행 전 사용량 한도에 걸림(화면 안내: 19:27 이후 재시도). 한도 안내 창에서 모델이 `GPT-6-Luna medium`으로 바뀐 상태로 보임(확인 필요). Codex에 gpt-6 계열이 있다는 것이 드러남 → 처음 말씀하신 "6.1sol"이 gpt-6 계열이었을 가능성(확인 필요). DD-11은 Sonnet으로 진행
- DD-11 Sonnet 1라운드: 아리아리 무효화 대조 26경로 중 빠짐 7·과잉 1, 모집 마감 시 프론트가 "CLOSED"를 직접 써 넣는 등 서버 파생 상태 재구현 → v2(읽기 의존 태그 + 쓰기 변경 태그, 동기 경로 한정 명시)
- 교차 정합성 검토(`reviews/CROSS-sonnet-r1.md`): P0 없음, P1 14건 반영. 확장 추가는 사람 확인 대상에서 제외(원문 승인 절차 금지), 확장 우회 방지 불변식의 보장 방식이 DD-06 답에 종속됨을 명시, 저장된 조회와 등록된 요청의 관계 정의, 질문 번호 체계 정리, 클라이언트 SDK 담당 문서(DD-13) 신설
- DD-14 Sonnet 1라운드: PoC도 DB 오류 원문을 응답에 넣고(`exec.rs` db.message()), 다른 테넌트 행은 본문 코드가 NOT_FOUND와 달라 존재 추측 가능(`codes.rs` TENANT_MISMATCH) 확인 → v2(존재 비노출 세 층, 운영에 개발 모드 없음, 문구는 코드+매개변수). PoC 부채로 기록
- 2차 교차 검토(`reviews/CROSS-sonnet-r2.md`): P0 0, P1 4 반영(호출자 요청 작성 API 담당을 DD-13으로, DD-13 오류 분류와 DD-14 category 일치, validUntil 정의 연결, 응답 형식 후속 문서를 DD-14로). 1차 P1 14건 반영 확인

</details>

## 최신 변경 이력

- 2026-10-03 창시자 통합 지침의 기준·산출물·미정 범위·실제 협업을 표시하고 이전 기록을 이력으로 전환.
