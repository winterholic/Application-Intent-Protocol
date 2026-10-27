# 제품 통합 검증 (2026-10-17)

## 2026-10-22 WBS 7.2.9 시스템 상태와 시청 더미 데이터 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.9 잔여 4개 구현 조각 모두 완료, 미검증과 한계는 아래)

**시스템 상태(`User.systemStatus`, 결정 D-P-SYSSTATUS).** `reference/slice14i_oracle.py`가 같은 MySQL·MongoDB와 실제 Redis 위에서 원본 `get_system_status()`와 서버를 차례로 돌려 요청 수치(`total_requests_1h`, `avg_response_time_ms`, `error_rate_1h`)와 연결 상태를 비교했다: `the system status matched the original in 10 cases`. Redis가 있을 때(요청 200·오류 7·응답 시간 3개), 카운터가 비었을 때, 오류 카운터가 깨졌을 때(읽던 곳에서 멈추고 앞의 값을 유지), Redis가 아예 없을 때(`redis: error`, 수치 0), 회원·게스트 거절이 같았다. 서버 지표(CPU·메모리·디스크)는 실행 기계가 macOS라 원본 값과 비교하지 못했고(서버는 `null`) 이름 모양만 비교했다. Linux 계산은 두 가지로 확인했다: python 컨테이너(psutil 7.2.2)에서 `/proc/stat`·`/proc/meminfo`와 psutil 값을 같은 순간에 읽은 기록으로 식이 같음(`host_figures_equal_psutil_on_a_linux_capture`: CPU 11.2, 메모리 18.1과 16038.1 MiB, 디스크 8.5), 같은 코드를 Linux 컨테이너에서 컴파일해 실제 읽기를 실행(`host_figures_read_this_linux_machine`, 값이 0~100 범위). 실서버 시험은 Linux에서 같은 값이 0~100 범위인지 확인하도록 썼지만 이 기계(macOS)에서는 실행되지 않았다(CI 결과로 확인, 미실행). 변이: 관리자 확인을 빼면 회원 호출이 `mysql: error`가 든 상태를 받아 실서버 시험이 실패했다.

**시청 더미 데이터(`generateDummyData`, 결정 D-P-DUMMY).** `reference/slice14j_oracle.py`: 프레임 생성기·통계는 시드 6개에서 원본과 같은 프레임이고 생성기가 같은 상태로 끝났다. 작업은 같은 데이터에서 먼저 포트(실행 id를 시드로)를 돌리고, 표를 되돌린 뒤 같은 시드로 원본 `generate_dummy_data`를 돌려 비교했다: `the test data matched the original in 20 cases`. 영상 선택과 순서(40개, 풀 60개), 세션 문서(타임라인·비율·완주율·0인 감정 초·`finalized_at` 없음·클라이언트 정보 없음, `created_at`은 두 실행의 간격만큼만 다름), 시청 기록 행(관리자, 세션의 시각), 영상마다 조회수 +1, 분포(평균·추천 점수·지배 감정·완주율)가 같았고 회원의 시작, 다른 회원 id, 개수 9와 301은 거절되며 같은 key는 같은 run이었다. 풀이 100을 넘어도(120개, 요청 300) 통과했다. 실서버 시험(`mysql_facereview_trending_job`): 요청 300개에 대해 보이는 영상 중 길이가 있고 2시간 이하인 30개만 만들어지고(삭제·길이 0·7201초 영상은 제외), 세션의 `user_id`가 관리자의 UUID이고, 프레임 수가 길이의 2배, 키가 0·50·100…, 비율 합 1, 지배 감정이 최대 비율, 시청 기록 행이 세션의 id를 가지고 시각이 90일 안, 조회수 +1, 분포가 원본 식을 세션 하나에 적용한 값, 같은 key가 같은 run임을 확인한다.

**변이 확인.** 오라클: 시각 난수 뽑는 순서를 바꾸면 세션 문서와 분포가 실패, 조회수를 빼면 실패, `happy` 추천 점수 계수를 바꾸면 분포가 실패, 지배 감정 신뢰 기준을 바꾸면 분포가 실패, 완주율을 바꾸면 세션과 분포가 실패했다. 실서버 시험: 작업 주체의 UUID를 풀지 않으면(수정 전 동작) 세션 소유자 검사(30개 기대, 0개)가 실패했다.

**발견한 기존 결함.** durable job이 Mongo의 개인 문서(`youtube_watching_data`)에 쓸 때 소유자를 UUID가 아닌 내부 정수 핸들(예: 2)로 적었다(job의 Mongo principal이 `actor_uuid`를 모름). 처음 오라클이 세션 0개로 이를 드러냈다. `crates/aip-service/src/mysql_jobs.rs`에서 작업 시작 때 행위자 UUID를 `aip_actor_identities`에서 풀어 쓰도록 고쳤다(자식 job은 이어받는다). 읽기·쓰기 확장은 원래 UUID를 썼다. 고친 뒤 job 시험 7종(각각 따로 실행)과 같은 정의의 실DB 시험이 모두 통과했다.

**알려진 차이와 한계.** (1) 더미 데이터는 비동기 job이다. 결과는 `/jobs/result`의 `created_count`와 `video_ids`(JSON 텍스트)이고 입력에 관리자 본인의 `user` id가 있다. 서비스는 `limits.job_calls`를 300개 영상에 2000 이상으로 설정해야 한다(영상당 5번). (2) job에는 saga가 없다: 한 영상 중간에 실패하면 앞 단계(시청 기록 등)가 남고 재시도가 나머지를 끝낸다. (3) 2시간(7200초)을 넘는 영상은 건너뛴다(세션 문서가 호출 하나의 1 MiB를 넘는다). 원본은 길이 제한이 없다. 건너뛰면 그 뒤 난수가 원본과 달라진다. (4) 서버가 `BAD_VALUE`로 거절하는 ctx 호출 하나(예: 조회수가 필드 최대인 영상에 조회수 +1)는 작업 전체를 끝낸다. 원본은 그 영상만 건너뛴다. 처음 실서버 시험이 이로 실패했다(앞 단계의 시험 영상이 조회수 40억을 남김). (5) 보이는 영상이 5000개를 넘으면 일부만 뽑지 않고 실패한다. (6) 분포 계산이 읽는 세션은 영상의 최신 1000개이고(원본과 같음), 어떤 시청자의 세션에 숫자가 아닌 값이 있으면 원본처럼 그 영상의 분포만 갱신하지 않는다(`WIRE.BAD_VALUE`를 job이 받지 못하고 호스트가 작업을 끝내는 규칙 때문에 이 경로는 실행해 보지 못했다: 미검증). (7) `VideoViewLog`에 공개 create가 생겼다(관리자가 자기 id로만, 시각 지정 가능). (8) 시청 기록의 `created_at`은 마이크로초까지 보내 MySQL이 반올림하며 오라클에서 1초 안으로 같았다. (9) 시스템 상태의 서버 지표는 Linux 서버에서만 값이 있다. 요청 수치는 배포가 Redis에 쓰기 전까지 0이다.

## 2026-10-22 WBS 7.2.9 관리자 대시보드 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.9는 진행 중)

**범위.** 원본 `AdminService.get_signup_trend`(기간 5개)와 `get_business_stats`(가입 추이, 주간 활동 회원, 요청 파이프라인, 콘텐츠 건강)를 옮겼다. 결정과 대안은 [DECISIONS](../docs/DECISIONS.md)의 D-P-DASHBOARD다. 새 언어 기능 없이 관리자 전용 단독 집계 4개(`User.signupCount`, `User.weeklyActive`, `VideoRequest.pendingCount`, `Video.categoryViews`)와 읽기 확장 2개(`VideoRequest.pipeline`, `Video.contentHealth`)를 더했고 플랫폼에는 Mongo 파생 컬렉션 `video_distribution_stats`(명령 `health`) 하나를 더했다. 읽기 확장은 `allow`가 없어서 첫 호출이 관리자 전용 집계이고, 비관리자는 거기서 `ACCESS_DENIED`로 끝난다(호출자는 `WIRE.EXTENSION_ERROR`).

**원본과의 대조(`reference/slice14h_oracle.py`).** 같은 MySQL 8.4와 MongoDB에서 오늘(UTC)을 둘러싼 3년 치 행(각 기간 경계의 첫 초와 마지막 초, 탈퇴 회원, 7일 경계의 시청 기록과 게스트 기록, 30일 안팎에서 처리된 요청 238개, 삭제·NULL 영상, 평균이 없는 분포 문서)으로 원본 서비스와 실제 `aip service serve`를 차례로 돌렸다: `the dashboard matched the original in 25 cases`. 가입 추이 6개(7d, 30d, 3m, 1y, 3y, 알 수 없는 기간)는 버킷마다 `signupCount`를 한 번 부르는 호출자 쪽 조립이 원본 JSON과 같았다. 이 밖에 7일 가입 추이, 주간 활동 회원, 대기 수, 장르별 조회수와 순서, 파이프라인(대기 수와 평균 처리 분), 콘텐츠 건강이 같았고, 회원과 게스트는 집계 4개에서 `WIRE.ACCESS_DENIED`, 확장 2개에서 거절(회원은 `WIRE.EXTENSION_ERROR`, 게스트는 `WIRE.UNAUTHENTICATED`)이었다.

**실서버 시험(`mysql_facereview_trending_job`의 새 단계, `1 passed`, 약 34초).** 오늘 기준 상대 시각 행으로 원본의 SQL과 비교한다(시각이 매번 달라 원본 대조 기록은 두지 않았다): 경계 14개 범위의 가입 수(끝 시각 제외, 탈퇴 회원 포함), 주간 활동 회원 4명(게스트·경계 밖 기록 제외, 한 회원이 두 번 봐도 한 번), 대기 수 2, 최근 30일 처리분 231건(100행씩 세 쪽)의 평균 처리 분(`AVG(TIMESTAMPDIFF(MINUTE ...))`를 원본처럼 한 자리로), 처리분이 없으면 평균 `null`, 장르별 조회수(삭제 영상 제외, 40억 조회수 합산), 기록한 분포 문서(`slice14h-health.json`)의 콘텐츠 건강, 회원·게스트 거절. 같은 정의를 쓰는 나머지 실DB 시험 7종도 통과했다.

**변이 확인.** 오라클: 가입 범위의 끝을 포함하면 6개 기간이 실패, 분을 반올림하면 파이프라인이 실패, 쪽 넘김을 빼면 파이프라인이 실패, 주간 활동에서 시청 기록 조건을 빼면 실패했다. 실서버 시험: 같은 조건 제거(44명), 파이프라인에서 `REJECTED`를 빼면(평균 229.5 대 229.0), 가입 집계를 모두에게 열면 각각 실패했다. 살아남은 변이 하나: `categoryViews`의 `is_deleted = false` 조건을 빼도 결과가 같았다. 집계가 `Video`의 행 정책(삭제 영상 제외)을 이미 적용하기 때문이며 조건은 명시용으로 남겼다(정의 문서의 "원본 행 정책을 물려받지 않는다"와 다르게 관찰되었고 원인은 확인하지 않았다).

**알려진 차이와 한계.** (1) 호출 횟수가 많다: 통계 화면 하나가 집계 수십 번(장르는 `Video.totalCount`로 존재 여부를 보고 `categoryViews`를 부르면 장르당 2번, 가입 추이 3y는 37번)이다. 확장이 집계에 호출자 입력을 넘기지 못하는 현재 규칙 때문이며, 그룹 집계나 distinct·평균 release는 새 언어 기능이라 결정 없이 만들지 않았다. (2) 기간과 기준 시각은 호출자가 정한다. 원본은 서버 시각이다. 가입 추이는 미래 시각 가입을 세지 않는다. (3) 원본의 Redis 캐시가 없다. (4) 파이프라인은 처리분을 최대 5000건(100행 50쪽)까지 읽고 넘으면 틀린 평균 대신 실패한다. 이 상한 초과는 시험하지 않았다. (5) 같은 수의 영상이 있는 지배 감정은 이름순이다(원본은 Mongo 순서). (6) `exists`가 `VideoViewLog`의 행 정책을 적용하지 않고 전 회원의 기록을 본 것은 시험으로 확인했지만(주간 활동 4명) 정의 문서의 설명과 맞는지는 확인하지 않았다. (7) 생성 SDK(`aip service gen`)에는 단독 집계의 타입 바인딩이 없다(`User.totalCount` 같은 기존 집계도 같다). 확장 2개의 설명자는 생성된다. 화면 호출은 7.2.10이다. (8) 시스템 상태와 더미 데이터는 아직 없다.

## 2026-10-22 WBS 7.2.9 영상 등록 요청 `home/video/recommend` (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.9는 진행 중)

**범위.** 원본 `HomeService.create_user_video_recommend`를 쓰기 확장 `VideoRequest.recommend`로 옮겼다. 결정과 대안은 [DECISIONS](../docs/DECISIONS.md)의 D-P-RECOMMEND다. 확장은 목록의 URL마다 기존 공개 create를 같은 트랜잭션에서 부르므로 소유자 검사, 필드 규칙, 유일 키가 create 자체의 것이다. `youtube_full_url`은 호출자가 쓰지 못하고(create의 `fields`에서 뺐다) DB 기본값이 계산하며, 전역 중복 거절은 `video_request.youtube_url` 유일 키(열은 대소문자를 구분하는 `utf8mb4_bin`)가 맡는다(`db-defaults.sql`). 플랫폼에 더한 것은 하나다: 거절된 create 뒤 확장이 같은 코드(`WIRE.ALREADY_EXISTS`)로, 또는 `WIRE.CONFLICT`로 끝나면 호출자가 그 코드를 받는다(`spikes/spike-v4-worker/src/write.rs`). 같은 규칙으로 감정 요약 확장의 첫 행 생성 경쟁이 `WIRE.CONFLICT`가 된다.

**원본과의 대조(`reference/slice14g_oracle.py`).** 같은 MySQL 8.4에서 사례마다 같은 시작 표로 원본 서비스와 실제 `aip service serve`를 차례로 돌려 결과(생성 또는 중복 거절)와 이후 표(소유자·id·전체 URL·상태·코멘트·시각이 최근인지)를 비교했다: `the video request batch matched the original in 23 cases`. 14개는 원본과 같아야 하는 사례다(새 id 하나·셋, 같은 회원·다른 회원·`ACCEPTED`·`REJECTED` 요청과 같은 id 거절, 새 id가 중복 앞뒤에 있을 때 하나도 남지 않음, 영상으로만 있는 id 허용, 공백·기호가 든 id를 그대로 보존, 50자 id, 대소문자만 다른 id는 다른 id, 20개). 나머지 9개는 이 확장 자신의 가드(빈 목록, JSON 아님, 객체, 숫자, 빈 id), 다른 회원의 id와 게스트 거절, 같은 key 재전송, 겹치는 목록 두 개의 동시 요청 10라운드다. 대소문자 사례는 처음에 실패했고(표의 `ai_ci` collation 아래 유일 키가 `CaseId`와 `caseid`를 같은 영상으로 거절) 열을 `utf8mb4_bin`으로 바꿔 고쳤다.

**실서버 시험(`mysql_facereview_trending_job`의 새 단계, `1 passed`, 약 32초).** 위 14개 사례를 기록(`slice14g-golden.json`)으로 다시 돌리고, 가드 7개(빈 목록, JSON 아님, 객체, 숫자, 빈 id, 21개, 열보다 긴 id)가 아무것도 만들지 않음, 다른 회원의 id·비활성 회원·게스트 거절, 같은 key 재전송이 두 번째에 아무것도 더 만들지 않음, 새 key로 같은 id를 보내면 `WIRE.ALREADY_EXISTS`, 서로 다른 두 회원이 겹치는 목록을 동시에 보내는 10라운드(매번 정확히 한 목록이 통째로 생기고 진 쪽은 `WIRE.ALREADY_EXISTS` 또는 `WIRE.CONFLICT`를 받으며 흔적이 없고 다시 보내면 `WIRE.ALREADY_EXISTS`, id마다 요청은 하나)를 확인한다. 같은 시험에 감정 요약의 첫 호출 동시 경쟁 6라운드(행은 하나, 진 호출은 `WIRE.CONFLICT`나 `WIRE.WORKER_BUSY`, 이어진 호출은 더하지 않음)를 더했다. 같은 정의를 쓰는 나머지 실DB 시험(`reads`, `admin`, `auth`, `watch`, `recommend`, `pool_job`, `category_fill_job`)도 모두 통과했다.

**변이 확인.** (1) `db-defaults.sql`에서 유일 키를 빼면 `an id the member requested before is refused`에서 실패했다(중복이 그대로 생성). (2) 서버의 `WIRE.ALREADY_EXISTS` 전달을 빼면 결정적 시험 `a_refused_create_reaches_the_caller_only_as_the_extension_passes_it_on`이 `WIRE.EXTENSION_ERROR`와 `WIRE.ALREADY_EXISTS`가 다르다며 실패했다. (3) 요약 확장의 `WIRE.CONFLICT` 변환을 빼도 실서버 시험이 통과했다: 관찰된 6라운드에서 진 호출이 유일 키 경로(`WIRE.ALREADY_EXISTS`)를 탔는지는 확인하지 못했다(시험은 진 호출의 코드를 `WIRE.CONFLICT`나 `WIRE.WORKER_BUSY`로만 허용하고 어느 쪽인지 기록하지 않았다). 이 변환은 서버 쪽 규칙(위 (2)의 시험)으로만 보증되고 확장 쪽 변환을 실행한 증거는 없다.

**호출 경로.** `aip service gen --lang ts`로 만든 바인딩에 `VideoRequest.recommend`가 쓰기 확장 설명자와 타입(`input: { user: Id<"User">; youtube_url_list: string }`, `output: { requested: number }`)으로 들어간다. 이 정의에서 `--lang python`은 `Json` 필드를 지원하지 않아 실패하는데 이 조각 이전의 정의에서도 같다. 생성 바인딩을 쓰는 화면 호출은 7.2.10이다.

**알려진 차이와 한계.** (1) 호출자가 자기 id를 `user`로 보낸다(모든 공개 create처럼, 서버가 호출자와 비교해 다른 id를 거절). 원본은 id를 받지 않는다. (2) 목록은 JSON 배열 텍스트(최대 1500자)이고 최대 20개다. 원본은 개수 제한이 없다. (3) 같은 id를 한 목록에 두 번 쓰면 원본은 두 행을 만들지만(표에 유일 키가 없다) 여기서는 거절하고 아무것도 만들지 않는다. (4) 열보다 긴(51자) id는 원본에서 DB 오류, 여기서는 확장 오류다. 입력 검증 실패는 `WIRE.BAD_VALUE`가 아니라 `WIRE.EXTENSION_ERROR`로 온다. (5) 중복 응답은 HTTP 409 V004가 아니라 `ok: false`와 `WIRE.ALREADY_EXISTS`다. (6) 운영 DB에 같은 id의 요청이 이미 둘 이상이면 유일 키를 만들 수 없다. `db-defaults.sql`의 점검 쿼리로 정리한 뒤 적용해야 하며 이 실행에서는 실제 운영 데이터를 보지 못했다. (7) 열을 `utf8mb4_bin`으로 바꿔 `video.youtube_url`(`ai_ci`)과의 비교가 대소문자를 구분한다. 승인 job이 만드는 `Video`의 유일 키는 여전히 `ai_ci`라서 `abc`와 `AbC`를 같은 영상으로 거절한다(이 조각 이전부터). (8) 확장 쪽 `WIRE.CONFLICT` 변환 실행 증거 없음(위 변이 (3)). (9) 이 조각은 MySQL 8.4에서만 실행했다. MariaDB 11.4에서 표현식 기본값이 다른 열을 참조하는 것만 SQL로 확인했고 서비스는 돌리지 않았다.

## 2026-10-22 WBS 7.2.8 조각 D: 마이페이지 감정 요약 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 마이페이지 읽기 6개 모두 구현)

**범위.** 원본 `MypageService.get_emotion_summary`(`GET mypage/emotion/summary`)를 쓰기 확장 `User.myEmotionSummary`로 옮겼다. 결정과 대안은 [DECISIONS](../docs/DECISIONS.md)의 D-P-EMOSUMMARY다. 플랫폼에 더한 것: (1) 쓰기 확장 `access`의 `read.<Resource>`·`create.<Resource>`와 `ctx.data.read`(호출자 행 정책을 쓰기 트랜잭션 안에서), `ctx.data.create`(공개 create 한 행), `ctx.data.apply(..., values, expect)`(patch 전이의 값과 낙관적 기대값). (2) 쓰기 확장 안의 Mongo·Redis 읽기 명령이 효과 기록 없이 즉시 실행(전에는 쓰기 경로로만 가서 `WIRE.CAPABILITY_DENIED`). (3) 확장이 `expect` 충돌을 그대로 올리면 호출자가 `WIRE.CONFLICT`를 받는다. (4) 파생 컬렉션 `user_watch_stats`의 명령 `emotion_seconds`: 체크포인트가 없으면 본인 세션 전체를 한 번(타임라인을 초 단위로, 마지막 프레임이 이김), 있으면 `finalized_at > t 또는 (= t 이고 video_view_log_id >)` 세션을 그 두 키 순서로 더해 소수 합계와 새 체크포인트를 돌려준다(세션 20000개 상한). 정의의 `UserEmotionSummary`에 공개 create와 전이 `applyDelta`(`patch` 합계·체크포인트, `expect lock_version`)를 더했고, `db-defaults.sql`이 합계·`lock_version`·시각의 기본값과 `last_finalized_at datetime(3)`을 둔다.

**원본과의 대조(`reference/slice14f_oracle.py`).** 같은 MySQL·MongoDB에서 사례마다 같은 시작 행으로 원본 서비스와 실제 `aip service serve`를 차례로 돌려 응답과 이후 행(합계·체크포인트·세션 id·`lock_version`)을 비교했다: `the emotion summary matched the original in 19 cases`. 행이 없는 첫 호출, 체크포인트 없는 행, 같은 `finalized_at`의 동점(세션 id 순서), 체크포인트가 기록의 앞·가운데·뒤·마지막 세션인 경우와 세션 id가 없는 경우, 저장된 초·타임라인(중복 초, 잘못된 감정·키)·비율×시청 시간(`duration`×`completion_rate` 상한 1, `timeline_len`, `frame_count`, 숫자가 아닌 `duration`)·문자열/실수 초, 소수 초 세션, 세션 800개(체크포인트 없이·가운데에서), 세션이 없는 회원, 반복 호출은 변화 없음, 늦게 끝난 세션은 정확히 한 번, 동시 두 호출(한쪽 `WIRE.CONFLICT`, 합계는 한 번), 게스트 거절. CI 재생: `mysql_facereview_trending_job`이 같은 세션(`slice14f-sessions.json`)과 시작 행으로 기록된 19개 답과 행(`slice14f-golden.json`)을 `/apply`로 재생하고, 한 회원이 4개 호출을 8번 동시에 보내도 합계가 한 번만 더해지고 한 번에 한 호출만 이기며(나머지 `WIRE.CONFLICT`, 로컬 확장 동시 호출 상한을 넘으면 `WIRE.WORKER_BUSY`), 전이의 낡은 `lock_version`은 거절되고 현재 값은 적용되며 다른 회원은 행을 바꾸지 못하고 게스트는 거절됨을 확인했다(`1 passed`, 약 24초).

**원본 결함과 고친 방법.** 원본은 `last_finalized_at`을 소수 초 없는 `DATETIME`에 저장해 소수 초가 `.5` 미만인 세션의 체크포인트를 내림하므로 다음 호출이 같은 세션을 다시 더한다. 원본에서 직접 측정했다(새 세션 1개, 호출 3번: 40, 50, 60초). `db-defaults.sql`이 열을 `datetime(3)`으로 바꾸면 원본 앱도 그대로 한 번만 센다. 이 포팅의 대조는 바꾼 열 위에서 한다.

**변이 확인.** 동점 규칙 `video_view_log_id >`를 `>=`로 바꾸면 "체크포인트가 첫 동점" 사례가 실패했고 복원했다. 낙관적 잠금(`expect lock_version`)을 정의와 확장에서 함께 빼도 시험이 통과했다: 동시 호출에서 MySQL 교착(1213)이 같은 `WIRE.CONFLICT`로 거절되어 이 시험은 `expect` 자체를 구별하지 못한다. `expect`의 판정은 전이 계약 시험(낡은 `lock_version` 거절)이 따로 확인한다.

**알려진 차이와 한계.** 자세한 내용은 [extensions/README](tests/fixtures/facereview-mysql/extensions/README.md)의 Slice14-F다. 요약: 전이 `applyDelta`는 처음엔 공개라서 회원이 자기 합계를 직접 쓸 수 있었으나 아래 후속(D-P-INTERNAL-APPLY)에서 `internal`로 막았다. 첫 호출끼리 경쟁하면 한쪽이 `WIRE.ALREADY_EXISTS`, 잠금 충돌은 3회 재시도 대신 호출자가 다시 보낸다, 세션 20000개 상한.

## 2026-10-22 WBS 7.2.8 조각 D 후속: 감정 합계 전이 노출 차단

**설계·범위.** [D-P-INTERNAL-APPLY](../docs/DECISIONS.md)는 기존 apply 엔진 위에 `internal` 노출 표식을 둔다. 공개 `/apply`와 atomic 경로는 이를 `WIRE.NOT_EXPOSED`로 거절하고 public compose는 정의 단계에서 막는다. Extension/job worker는 기존 `access` 검사를 거친 `apply_in` transaction에서만 허용한다. public generated binding에는 해당 동작을 넣지 않는다.

**방금 실행한 검증.** `CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target cargo test -p spike-v1-fixture --test internal_apply --locked` → 2 passed. `AIP_TEST_DATABASE_URL='host=127.0.0.1 port=34464 dbname=aip_test_run user=runner password=…' AIP_TEST_ADMIN_URL='host=127.0.0.1 port=34464 dbname=postgres user=runner password=…' CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test internal_transition --locked -- --nocapture --test-threads=1` → 1 passed: 실제 PostgreSQL에서 direct `/apply`, atomic 단독·허용 단계 뒤 internal 전이 거절, 같은 public 전이 허용, internal owner 거절·낡은 `expect`의 `WIRE.CONFLICT`·정상 갱신을 확인했다. 별도 `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib duplicate_create_conflict_tests::duplicate_first_row_creation_is_returned_as_a_retryable_conflict --locked -- --nocapture` → 1 passed: scripted extension worker가 create에서 `WIRE.ALREADY_EXISTS`를 받고 `WIRE.CONFLICT`로 종료하는 실제 worker completion 루프를 실행했다.

**미검증.** `crates/aip-cli/tests/mysql_facereview_trending_job.rs`의 MySQL+Mongo 실서버 시험은 변경 직후 실행했으나, `aip service serve` 기동 단계가 `SERVER_CONFIG` / `io other error` (worker runtime check, readiness store 설치, listener 중 정확한 하위 원인은 출력되지 않음)로 종료되어 `WIRE.NOT_EXPOSED`의 MySQL HTTP 경로와 해당 시험 내부 전체 감정 요약 재생은 실행하지 못했다. 실행 환경은 이 Run의 macOS shell에서 띄운 격리 fixture/test process이며 배포 backend 결과가 아니다. 실행 명령은 `AIP_FIXTURE_TAG=aipresume226`, MySQL `127.0.0.1:34461`, Mongo `127.0.0.1:34463`, `cargo test -p aip-cli --features mysql --test mysql_facereview_trending_job --locked -- --ignored --exact the_trending_job_saves_new_videos_with_their_initial_documents --nocapture --test-threads=1`이다. 에러는 서비스 시작 전 단계의 관측이며 원인 추정·운영 가용성 판정으로 확대하지 않는다. 첫 행 create 경쟁에서의 MySQL HTTP 응답별 ALREADY_EXISTS→CONFLICT도 아직 미검증이다.

## 2026-10-22 WBS 7.2.6 원본·재현환경 사전 조사

고정 backend는 `7948b7e721837fdb73c427cb54e12cad4ec2d5af`, UI는 `2e7c021fa5abebd72278d841b8066a33d45cfbb3`다. 두 local checkout은 변경이 있으므로 baseline 기준은 working tree가 아닌 pinned `HEAD`와 `facereview-inventory.json` SHA다. Pinned `watch_frame`은 authenticated Socket.IO 연결을 사용하지 않고 body의 `user_id`를 신뢰하며, 첫 프레임에서 cache·RDB view log를 만들고 모델 inference와 timeline update를 수행한다. disconnect는 log만 남기며 종료·재집계하지 않는다. UI는 0.5초마다 `socket.io-client` websocket transport로 frame을 보내고 초 단위 2 decimal running time으로 callback 응답을 읽는다.

`common/ml/model.h5`와 저장소 루트 `model.h5`는 동일 SHA-256 `c10287ebb2781f10f23acffb122aef8e4ce7efc5e6ea6269522b1669a1aa00b0`, 각 378 MB이고 owner-only mode `0700`이며 Git ignored다. 저장소 manifest에는 포함되지 않아 재현 package로 보장되지 않고, 사용 권한의 provenance는 아직 확인되지 않았다. Pinned source는 TensorFlow 2.15.0·OpenCV 4.9.0.80·NumPy 1.26.3을 요구하지만 기존 `baseline-api.lock`은 이 ML 의존성을 포함하지 않으며 `/tmp/aip-facereview-baseline-venv`도 현재 없다. 그래서 baseline server+real inference 재현은 미실행이다.

`projects/facereview` source checkout scan에서는 실제 frame/video 테스트 자료를 찾지 못했다(로고·페이지 screenshot만 확인). 이 범위 밖에 사용 허가된 media가 더 있는지는 미확인이다.

진행 후보는 D-P-WATCH-TRANSPORT의 5개 세로 조각이다: (1) auth/transport/payload/disconnect, (2) idempotent first-frame session+view log, (3) pinned specialized ML runtime의 real inference, (4) Mongo timeline/aggregate persistence, (5) original backend를 끈 실제 브라우저 player/camera trace. 가짜 점수나 unit/socket test를 ML E2E로 세지 않는다. 모델 provenance와 재현 가능한 TF runtime은 게이트로 남는다. WBS 7.2.1.3의 MySQL event subscription 및 `events_days` cleanup도 별도 항목으로 계속 추적한다.

현재 tree를 처음 읽었을 때 production/runtime 코드와 회귀시험은 이미 uncommitted로 존재했고, 그 변경의 선행 TDD RED 명령·출력은 이 Run에서 회수되지 않았다. 이번에 추가한 worker conflict test의 첫 실행은 테스트 harness 결함(호출 메시지 필드·facts input shape)을 잡았고, 올바른 fixture로 수정 뒤 1 passed다. 따라서 이전 변경의 TDD RED가 있었다고 주장하지 않는다.

## 2026-10-22 WBS 7.2.8 후속: applyDelta 공개 차단의 MySQL 종단 실행 (로컬 재실행, 앞 절의 미검증을 갱신)

앞 절 「미검증」의 MySQL+Mongo 시험을 이 Run의 macOS shell(격리 fixture: MySQL 8.4 `127.0.0.1:36451`, MongoDB 7 인증 `36453`, Redis 7.4 `36454`, PostgreSQL 17 `36473`, 모두 이 Run 소유 컨테이너)에서 다시 실행했다. 명령은 `cargo test -p aip-cli --features mysql --test mysql_facereview_trending_job --locked -- --ignored --exact the_trending_job_saves_new_videos_with_their_initial_documents --nocapture --test-threads=1`이고 결과는 `test result: ok. 1 passed; 0 failed` (46.9초)다. 이 시험 안에서 `aip service serve`가 정상 기동했고, 직접 `UserEmotionSummary.applyDelta` apply·expect 없는 apply·atomic 단독·허용된 변경 뒤 중첩 atomic 5가지가 모두 `WIRE.NOT_EXPOSED`로 거절됐으며(시험 코드의 단정, 약 888~908행), 확장 `User.myEmotionSummary`의 정상 갱신과 원본 골든 재생이 같은 실행에서 통과했다. 이것은 로컬 macOS 실행이고 Linux CI와 배포 backend 결과가 아니다. 배포 환경은 미실행이다.

앞 실행의 `SERVER_CONFIG` / `io other error`는 재현하지 못했고 원인은 여전히 확정하지 못했다. 통제 실험 한 가지를 했다: 앞 실행의 명령 기록에 `AIP_TEST_REDIS_URL`이 없었으므로 그 변수만 뺀 같은 명령을 다시 돌렸고 같은 `1 passed`였다(`redis_url`이 빈 문자열이어도 이 시험은 Redis를 쓰지 않는다). 따라서 Redis 미설정 가설은 기각됐다. 남은 후보(앞 실행의 fixture 포트·컨테이너 상태, 그 시점의 worker runtime 점검 실패 등)는 그 환경이 회수되어 검증하지 못한다. 같은 오류를 무작정 다시 돌리지 않았고 다시 나타나면 `server_start_error`가 숨기는 하위 단계(`crates/aip-service/src/lib.rs`)를 로그로 드러내야 한다. CI 쪽 근거: `40b4539` 커밋의 Linux worker jobs와 Database contracts는 success이지만 이 두 workflow는 위 시험을 포함하지 않는다. 이 시험은 Verify workflow의 `scripts/verify-mysql-worker-e2e.sh run-ci`가 돌리며, `40b4539`와 `ab2f79f`의 Verify는 뒤 커밋 push로 취소됐다. 이 절의 코드를 모두 포함한 `58e1fdb`(7.2.6 조각 1 커밋)의 Verify(run 39788184086)는 success이고, 그 로그에서 `the_trending_job_saves_new_videos_with_their_initial_documents`가 Linux runner(Bubblewrap worker)에서 `test result: ok. 1 passed; 0 failed ... finished in 46.35s`로 통과했다. 따라서 applyDelta 공개 차단과 `myEmotionSummary` 갱신의 MySQL 종단 실행은 로컬 macOS(46.9초)와 Linux CI 양쪽에서 확인됐다. 배포 backend 실행은 별개로 미검증이다.

## 2026-10-22 WBS 7.2.6 조각 1: Socket.IO 호환 전송 경계 (구현 조각, 로컬 통과, 7.2.6은 진행 중)

**범위.** 결정 [D-P-WATCH-SOCKET](../docs/DECISIONS.md). `spikes/spike-v6-transport/src/socketio.rs`가 Engine.IO v4(WebSocket 전용)와 Socket.IO v5의 부분집합(기본 namespace, CONNECT·DISCONNECT·평문 EVENT·ACK, 서버 ping/pong)을 말한다. 서버 연결은 `lib.rs`의 `/socket.io/?EIO=4&transport=websocket`, 토큰 검사는 기존 브라우저 WebSocket과 같은 `realtime::admit_browser_token`(이 조각에서 `stream_ws`의 인라인 closure를 함수로 뺐다), 설정은 서비스 설정 `socketio`와 `Infrastructure.socketio`(기본 꺼짐)다. `watch_frame`은 필수 필드·타입·범위만 검사하고 통과해도 ack는 `WIRE.NOT_AVAILABLE`이다. **감정 분석, 세션·view log 저장, Mongo timeline은 이 조각에 없다.** 그래서 이 조각은 7.2.6의 실제 영상·웹캠·ML 흐름을 하나도 증명하지 않는다.

**실행한 검증(로컬 macOS, 모두 이 Run의 격리 PostgreSQL 17 `127.0.0.1:36473` 사용).**

| 명령 | 결과 |
|---|---|
| `cargo test --lib socketio --locked` (spike-v6, 구현 전 `todo!()` 뼈대) | 22 FAILED (RED: 구현 전) |
| 같은 명령, 구현 뒤 | 이후 쿼리 검증·옵션 시험 2개를 더해 24개 통과(전체 실행의 일부) |
| `cargo test --test socketio_transport --locked -- --test-threads=1` | 4 passed: 설정 없을 때 미서비스, Origin·쿼리·Authorization 거절과 `permessage-deflate` 제안 수용, 실제 principal 검사(없는 subject는 `44` 뒤 종료)·연결 상한·frame ack, disconnect·초과 패킷(close 1009) 뒤 슬롯 반환 |
| 변이: 서버가 `Authorization` 헤더 토큰을 거절하는 조건을 지움 | `the_upgrade_accepts_only_a_browser_websocket_only_engine_io_v4_request` FAILED, 복원 뒤 4 passed |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test socketio_transport -- --ignored` | 1 passed: 실제 `socket.io-client` 4.8.3(원본 UI `yarn.lock`이 잠근 버전, `transports:['websocket']`, `auth` 콜백)이 거절된 토큰의 `connect_error`(message·data.code), 정상 연결 id, 유효 frame ack, 누락 필드 ack, 알 수 없는 이벤트 ack, 클라이언트 `disconnect()`(`io client disconnect`)를 관찰했다 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target` (전체) | `EXIT=0`, `test result: ok` 76개 묶음, FAILED 없음 (realtime 리팩터링 회귀 확인) |
| `cargo test -p aip-service --lib --locked` | 41 passed (새 `socketio` 설정 시험 포함) |
| `cargo fmt --all --check`, spike-v6 `cargo clippy --all-targets -- -D warnings`, aip-auth·aip-migrate·aip-service·aip-cli(`mysql`) clippy | 모두 경고 없음 |

**소스 대조.** 공식 문서(socket.io Engine.IO v4·Socket.IO protocol)를 읽고 패킷 부호화·open 패킷·ping 방향·CONNECT 선행 규칙을 따랐다. 원본 UI(`2e7c021…`)의 `socket/index.ts`와 `WatchPage.tsx`의 emit·ack 모양, 고정 backend(`7948b7e…`)의 응답 모양을 확인했다. 원본이 payload `user_id`를 신뢰하는 결함은 복제하지 않았다.

**한계·미검증.** (1) 실제 브라우저(Chrome 등)로 붙는 시험은 하지 않았다. `permessage-deflate` 거절 문제는 Node `ws` 클라이언트가 아니라 브라우저가 항상 제안하는 값이라서 수용 경로를 서버 시험으로만 확인했다. (2) 기존 `/events/socket`이 확장 제안을 거절하는 동작은 이번에 고치지 않았고 실제 브라우저 영향은 미확인이다. (3) 토큰은 CONNECT에서 한 번만 검사하고 연결은 토큰 만료 시각까지 유지한다. 연결 중 principal을 비활성화해도 이 조각은 즉시 끊지 않는다. 프레임별 재검사는 쓰기 경로를 정하는 후속 조각에서 한다. (4) 연결 상한은 `realtime.max_connections`를 공유하고 기본 32다. 운영 값은 정하지 않았다. (5) MySQL 서비스(`aip service serve`)에 이 endpoint를 켜는 경로는 설정 구조만 만들었고 MySQL 서버 위의 종단 실행은 하지 않았다(PostgreSQL 기반 spike 서버에서만 실행). (6) CI: 이 조각의 커밋 `58e1fdb5c47a9f3aff11ab7368eedc0631462646`은 동일 SHA의 8개 workflow(Verify 39788184086 포함)가 모두 success다. Verify 로그에서 `tools/verify.sh`의 새 단계(`npm ci --ignore-scripts` 뒤 `--test socketio_transport -- --ignored`)가 `the_real_socketio_client_connects_emits_watch_frame_and_disconnects ... ok`로 Linux runner에서 실행된 것을 확인했다. (7) 모델 출처·ML 런타임·고정 미디어 문제는 그대로이며 가짜 감정값은 만들지 않았다.

## 2026-10-22 WBS 7.2.6 조각 2: 첫 프레임의 세션·VideoViewLog 멱등 생성 (구현 조각, 로컬 통과, 7.2.6은 진행 중)

**범위.** 결정 [D-P-WATCH-SESSION](../docs/DECISIONS.md). 유효한 `watch_frame`이 선언된 쓰기 확장 `User.startWatching`(`extensions/slice14k_watch.py`)을 불러 `VideoViewLog` 한 행과 Redis 시청 세션(`facereview:watch:active:<view log id sha256>`, TTL `duration × 1.5`초 또는 10800초)을 만든다. 소켓은 프레임마다 토큰을 다시 검증해 거기서 나온 사용자 id만 쓴다. 쓰기 확장 호출 코드는 `/extension` 쓰기 경로에서 `run_relational_write_extension`으로 빼 두 곳이 같은 코드를 쓴다(복사하지 않았다). **세션을 만든 프레임의 ack도 `WIRE.NOT_AVAILABLE`이다. 감정 분석과 Mongo timeline이 없어 감정값을 만들지 않는다.** 7.2.6의 실제 영상·웹캠·ML 흐름은 여전히 하나도 증명되지 않았다.

**실행한 검증(로컬 macOS, 이 Run 소유의 격리 MySQL 8.4 `127.0.0.1:15461`, Redis `15464`, MongoDB `15463`, PostgreSQL 17 `15466`).**

| 명령 | 결과 |
|---|---|
| `cargo test -p aip-cli --features mysql --test mysql_facereview_watch --locked -- --ignored --nocapture --test-threads=1` (실제 `aip service serve`와 WebSocket) | 구현 중 실패를 차례로 드러냈다(Redis 쓰기의 `effect_key` 거절 `WIRE.BAD_VALUE`, 시험의 열 이름 `video_view_log_id`, Redis 키가 base64가 아닌 hex라는 불일치). 고친 뒤 `test result: ok. 1 passed` (11.8초) |
| 같은 시험 안의 단정 | 첫 프레임이 view log 1행과 Redis 세션을 만든다(소유자는 페이로드의 거짓 `user_id`가 아니라 토큰의 Alice). 같은 연결의 중복 프레임은 행 1개로 유지된다. 연결 해제 뒤 재접속(새 sid)은 같은 행을 쓰고 Redis 세션을 다시 채운다. 다른 회원(Bob)의 같은 id는 `WIRE.ALREADY_EXISTS`이고 행은 1개다. 연결 중 `principal revoke`된 회원(Carol)의 다음 프레임은 `WIRE.UNAUTHENTICATED` 또는 `WIRE.TOKEN_EXPIRED`로 거절되고 연결이 끝나며 행이 생기지 않는다. 만료 2초짜리 토큰의 연결은 만료 시각에 `WIRE.TOKEN_EXPIRED` close로 끝난다 |
| `cargo test -p aip-cli --features mysql --test <각각> --locked -- --ignored --test-threads=1` (trending_job·admin·reads·auth·recommend·pool_job·category_fill_job·external_effects) | 모두 `1 passed`. 정의에 새 확장이 생겨 시험 각각의 확장 목록에 `slice14k_watch.py`를 더해야 했다(안 더한 첫 실행은 `JOB_CONFIG`로 시작 실패). `VideoViewLog.create`의 `fields`에서 `updated_at`을 지웠을 때 더미 데이터 job이 깨질 것이어서 유지했다 |
| `python3 -m unittest test_slice14k_watch` (extensions 디렉터리) | `Ran 2 tests ... OK` (재접속의 재사용·다른 회원 거절·세션 불일치, 모의 ctx) |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --lib socketio` | 25 passed (프레임 핸들러 호출 여부, 토큰 만료로 인한 종료 추가) |
| `cargo test … --test socketio_transport -- --test-threads=1 --include-ignored` | 5 passed (PostgreSQL 서버에서는 유효 프레임이 `WIRE.ADAPTER_UNSUPPORTED`이고 실제 `socket.io-client` 4.8.3 종단도 통과) |
| `cargo fmt --all --check`, spike-v6와 aip-migrate·aip-service·aip-cli(`mysql`) `clippy --all-targets -D warnings` | 경고 없음 |

**소스 대조.** 원본 `video_watching_socket.py`(고정 `7948b7e…`)의 첫 프레임 처리 순서(캐시가 없으면 `init_watching_data` → 시청 기록 생성)와 `WatchingDataCache`의 `duration × 1.5`초(없으면 10800초) TTL을 옮겼다. 원본의 두 결함은 복제하지 않았다: 인증 없이 페이로드 `user_id`를 믿는 것, 프로세스 메모리에만 있어 재시작·다른 프로세스에서 세션이 사라지는 것.

**한계·미검증.** (1) 감정 분석·Mongo timeline·집계는 없다. (2) 회원이 `/apply`로 자기 이름의 view log(임의 id·시각)를 직접 만들 수 있다(결정의 알려진 한계). (3) 같은 연결의 프레임마다 DB 멱등 조회(저장 결과 재생)가 한 번 일어난다. 부하 시험은 하지 않았고 상한은 정하지 않았다. (4) 실제 브라우저·카메라 입력, 연결 수 상한 운영값, 프레임별 재인증의 비용은 미검증이다. (5) 변이 검사는 이 조각에서 하지 않았다(조각 1의 헤더 토큰 거절 변이만 있다). 폐기된 principal의 프레임 거절은 라이브 시험이 직접 보인다. (6) CI: 조각 2 커밋 `7f0f702a55303b3b1154430dc98a13d785754556`의 Linux worker jobs·App packages·Linux cgroup v2·External brokers·Database contracts·Object storage·Deployment checks 7종은 success다. Verify(run 39797221463)는 conclusion이 cancelled다: `core` job의 모든 시험 단계(단계 10 약 20분 30초, 단계 11 약 7분)가 success로 끝난 뒤 job이 시작 30분 7초 지점에서 `timeout-minutes: 30`에 걸려 마무리 단계에서 취소됐다. 시험 실패는 없었지만 workflow 결론이 success가 아니므로 success로 세지 않는다. 시험이 늘어 30분 상한에 닿은 것이라 `verify.yml`의 상한을 45분으로 올렸다(다음 push의 Verify로 확인한다).

## 2026-10-23 WBS 7.2.6 조각 3: 프레임 타임라인 저장 경로 (ML 미연결, 구현 조각, 로컬 통과)

**범위.** 결정 [D-P-WATCH-FRAMES](../docs/DECISIONS.md). 쓰기 확장 `User.recordFrame`(`extensions/slice14l_frame.py`, 입력 `{id, centi}`)이 프레임의 감정 점수를 Mongo `youtube_watching_data`의 `most_emotion_timeline.<centi>`와 `emotion_score_timeline.<centi>`(점수 순서 neutral, happy, surprise, sad, angry)에 `$set`으로 쓰고, 문서가 없으면 원본의 기본값으로 만든다. **감정 분석(ML)은 연결되지 않았다.** 점수는 입력이 아니라 서버만 쓰는 Redis 키 `facereview:watch:frame:<view log id sha256 hex>:<centi>`에서 `getdel`로 한 번 꺼내 쓴다(회원이 자기 세션과 모든 시청자 공용 타임라인을 위조하지 못하게 하기 위함). 이 키를 쓰는 서버 코드는 아직 없고 소켓은 이 확장을 부르지 않는다. 그래서 키가 없으면 `{recorded:false}`로 아무것도 쓰지 않으며 프레임 ack는 `WIRE.NOT_AVAILABLE` 그대로다. **아래 시험의 점수는 시험 코드가 Redis에 직접 쓴 고정 숫자이고 실추론이 아니다. ML E2E로 세지 않는다.**

**실행한 검증(로컬 macOS, 이 Run 소유의 격리 MySQL 8.4·MongoDB 7·Redis 7.4).**

| 명령 | 결과 |
|---|---|
| `python3 -m unittest test_slice14l_frame` (확장 디렉터리, 모의 ctx) | 구현 전 6 ERROR(모듈 없음)였고 구현 뒤 `Ran 6 tests ... OK`: 원본 점수 순서·동점 규칙, 점수 없으면 쓰지 않음, 남의 세션은 `session not found`이고 점수를 꺼내지 않음, 같은 키를 다시 쓰면 같은 자리·순서가 다른 키도 각자 자리, 잘못된 점수 8종 거절, 범위 밖 입력 거절. 소유 확인을 지우는 변이는 이 단위 시험을 FAILED로 만들었다 |
| `cargo test -p aip-cli --features mysql --test mysql_facereview_watch --locked -- --ignored --nocapture --test-threads=1` (실제 `aip service serve`, `/apply`) | `test result: ok. 1 passed` (12초). 점수 없음은 `{recorded:false}`이고 세션 문서가 생기지 않는다. `scores` 필드를 얹은 호출은 `ok:false`다(선언된 입력이 아님). 점수를 두면 문서의 `user_id`는 토큰의 Alice이고 video·duration은 view log 행과 `video` 표의 값이며 `frame_count`·`emotion_sum`은 쓰이지 않는다. 같은 센티초를 다시 넣으면 한 칸이 덮어써지고 더 이른 시각이 늦게 와도 각자 칸을 가져 도착 순서대로 2칸이다. 문서는 1개다. Bob이 Alice의 id로 부르면 `ok:false`이고 Alice의 문서는 그대로이며 점수 키가 남아 있다(보상). 형식이 틀린 점수는 거절되고 문서는 그대로다 |
| 같은 시험에서 소유 확인을 지우는 변이 | **시험이 잡지 못했다.** Bob의 호출이 확장의 확인 없이도 거절됐다: Mongo capability가 filter에 인증 principal의 `user_id`를 강제하고 `video_view_log_id` 유일 인덱스가 같은 id의 두 번째 문서를 막아 upsert가 실패하며, 보상이 점수 키를 되돌린다. 확장의 확인은 첫 방어선(아무것도 건드리기 전 거절)이고 위 두 가지가 두 번째 방어선이다. 첫 방어선은 단위 시험이 보인다 |
| live 시험 8종(`trending_job`·`admin`·`reads`·`auth`·`recommend`·`pool_job`·`category_fill_job`·`watch`)과 `mysql_external_effects`를 정의 변경 뒤 재실행 | 모두 `test result: ok. 1 passed` (정의에 새 확장이 생겨 각 시험의 확장 목록에 `slice14l_frame.py`를 더했다) |

**시험 환경의 주의.** 원본 골든 세션 문서에는 `video_view_log_id`가 없어서 서버의 `ensure_indexes`(유일 인덱스)가 실패한다. 시험은 부분 유일 인덱스(문자열 `video_view_log_id`가 있는 문서만)를 직접 만든다. 운영에서는 원본 앱이 만든 유일 인덱스가 이미 있다고 가정했고 그 DB는 확인하지 않았다.

**알려진 한계·미검증.** (1) 점수를 쓰는 서버 코드, 평균 감정(`average_emotion`)의 출처, 세션 확정과 `frame_count`·`emotion_sum`·`emotion_percentages`·`dominant_emotion`·`completion_rate` 계산, `video_distribution` 갱신은 없다. 원본 소켓 흐름은 세션을 확정하지 않는다(고정 소스에서 `finalize`의 호출처를 찾지 못했다). (2) 프레임마다 쓰기 확장 한 번(워커 기동 포함)의 비용은 측정하지 않았다. (3) 보상은 점수 키만 되돌리고 Mongo 쓰기는 멱등이라 되돌리지 않는다(큰 문서의 pre-image를 읽지 않기 위함). (4) CI: 이 조각의 커밋 `7c3ec334bd8585612de4b93e00dce4787d0bae7b`는 동일 SHA의 8개 workflow가 모두 success다. Verify(run 39802081466, job 22분)의 로그에서 `a_browsers_extension_offer_is_declined_not_refused ... ok`, 실제 `socket.io-client` 종단 `... ok`, Python 단위 시험(`Ran 8 tests`), Linux runner의 `mysql_facereview_watch` 실행(`recordFrame` 단정 포함)을 확인했다. 앞선 `7f0f702`의 Verify가 30분 상한에 걸려 cancelled였던 것은 이 실행으로 해소됐다(같은 시험이 22분에 끝났다).

## 2026-10-23 WBS 7.2.6 조각 4: 첫 프레임 Mongo timeline 초기화 (모델 미연결)

**범위.** 조각 2의 `User.startWatching` 호출은 그대로 유지한다. 따라서 매 프레임 재검증한 토큰에서 파생된 회원만 `VideoViewLog` 1행과 Redis 세션을 만들고, 같은 연결의 반복 프레임·새 연결 재접속도 같은 id의 행을 재사용한다. 이어 `User.recordFrame`을 서버의 `watch_frame` 경로에 연결했다. staged 점수가 없을 때에도 `youtube_watching_data`의 `video_view_log_id` 고유 키로 upsert해 Mongo 세션 문서를 초기화하며 `emotion_score_timeline`과 `most_emotion_timeline`은 빈 객체로 둔다. 같은 세션·시각의 반복 호출은 같은 문서에서 멱등 처리된다. 점수가 있는 경우 기존 server-only `getdel` 경로와 `$set`은 유지한다. 브라우저는 점수 입력을 보내지 못하고 actor id는 token에서 강제된다.

**보류 범위.** 이 변경은 `model.h5`를 열거나 inference를 실행하지 않는다. 빈 감정 맵을 영상 결과로 표시하지 않도록 ack는 `WIRE.NOT_AVAILABLE`을 유지하며 응답에 모델 연결 부재를 명시한다. visitor policy는 정의·선택하지 않았다. 세션 종료/finalization, `frame_count`, `emotion_sum`, 평균·분포 갱신, 실제 브라우저 미디어 재생·카메라 흐름은 구현·검증하지 않았다.

**검증.** 새 empty-timeline 단위 단정은 먼저 기존 코드에서 예상대로 실패했다(`len(ctx.mongo.updates)`: 0, expected 1). 구현 뒤 아래가 통과했다.

| 명령 | 결과 |
|---|---|
| `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest test_slice14l_frame` (확장 디렉터리) | `Ran 6 tests ... OK`; 점수 없음의 빈 timeline 초기화·재호출, staged 점수 경로·소유 거절·입력 검증 포함 |
| `env -u PGHOST -u PGPORT AIP_TEST_DATABASE_URL='host=127.0.0.1 port=54321 dbname=aip_test_ci user=runner' AIP_TEST_ADMIN_URL='host=127.0.0.1 port=54321 dbname=postgres user=runner' cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --lib socketio::tests:: --locked` | `25 passed; 0 failed` (Socket.IO 단위 25개) |
| `cargo fmt --all -- --check` | 통과 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --lib --locked` (전체 lib) | `77 passed; 21 failed`; 실패 21개는 PostgreSQL을 요구하는 테스트가 임시 DSN의 `127.0.0.1:54321`에 연결하며 `Connection refused`. 이 로컬 전체 suite 결과는 통과로 계산하지 않는다 |
| `cargo test -p aip-cli --features mysql --test mysql_facereview_watch --locked -- --ignored --nocapture --test-threads=1` | 코드에 회귀 단정을 추가했으나 로컬 실행은 미실시. 실제 MySQL/Mongo/Redis 종단 검증은 push 뒤 Verify에서 확인 예정 |

실제 종단 단정은 새 Verify에서 첫 프레임·중복 프레임·재접속 뒤 `VideoViewLog` 1행과 owner-scoped Mongo timeline 문서 1개, 두 감정 맵이 비어 있는지 확인하도록 추가했다. Mongo fixture 미실행 상태이므로 이 결과는 아직 미검증이다.

**이전 회원 검증 run.** Verify `39901115868`, SHA `6b1a8d4f70688caa9a61ed32bba6095c86fae7f9`은 2026-10-23 04:47 UTC에 `in_progress`였다. core step 10이 실행 중이고 pinned React profile browser·HTTP/worker/MySQL·artifact 처리 step은 pending이었다. 따라서 profile/reset/withdraw의 최종 결과는 아직 판정되지 않았고 artifact 생성·삭제도 하지 않았다. 이 run의 별도 검증 범위는 새 SHA의 증거로 쓰지 않는다.

다음 재개점은 새 SHA의 Verify에서 `mysql_facereview_watch`를 확인한 뒤, `39901115868`이 끝나면 기록된 안전 절차대로 raw logs/safe evidence만 확인하고 screenshot 포함 artifact를 삭제하는 것이다. profile browser, withdrawal cancel 미호출, artifact 삭제는 실행 전까지 미검증으로 남긴다.

## 2026-10-23 WBS 7.2.6 ML 게이트 조사: `model.h5`의 출처·허가와 재현 환경 (조사만, 구현 없음)

**읽은 곳**: 고정 backend `7948b7e…`의 저장소(`projects/facereview/new-facereview`, 작업 트리에 변경이 있어 파일 내용은 HEAD와 같은지 따로 확인하지 않았다), 그 git 이력, UI `2e7c021…`. 모델 파일은 읽기 전용으로 헤더와 메타데이터만 보았고 복사하지 않았다(임시 가상환경에서 `h5py`로 구조만 읽고 지웠다).

**사실.**
- 파일: `model.h5`와 `common/ml/model.h5`는 같은 바이트(396,829,992바이트, SHA-256 `c10287ebb2781f10f23acffb122aef8e4ce7efc5e6ea6269522b1669a1aa00b0`를 이번에 다시 계산해 둘이 같았다), 수정 시각 2025-11-28, HDF5 서명 확인. 저장소에 추적된 적이 없다(`git rev-list --objects --all`에 `.h5` 객체 없음, `git log -S model.h5`의 6개 커밋은 문서·CI·Docker 변경이며 파일 자체를 더하지 않는다). `.gitignore`가 `*.h5`를 막고 `docker-compose.yml` 주석이 "git/이미지에 없음, 서버에 scp로 올려둘 것"이라고 적는다.
- 구조: Keras 2.4.0·TensorFlow 백엔드로 저장한 Functional 모델, 층 177개(Conv2D·BatchNormalization·ReLU·Add가 반복되는 잔차 블록, ResNet 계열), 입력 `[None, 96, 96, 1]`, 손실 `categorical_crossentropy`, 최적화기 Adam의 가중치까지 들어 있다. 클래스 5개(`happy, surprise, angry, sad, neutral` 순서는 `emotion_analyzer.py`)로 읽히나 출력층은 끝까지 확인하지 않았다.
- 출처·허가: 저장소 어디에도 모델의 학습 데이터, 학습 코드, 라이선스, 출처 문서가 없다. `LICENSE` 파일이 없고 `README.md`·`CLAUDE.md`는 "얼굴 감정 분류" 이상을 적지 않는다. 2023년 팀 시절 코드(`origin-codes/*.py`, 수정 시각 2023-11~12)도 `keras.models.load_model('model.h5')`로 현재 디렉터리의 파일을 읽을 뿐이다. 커밋 작성자는 `winterholic`(116)과 `대선`(2)이다.
- 이름과 입력이 이 서비스에 맞게 맞춰진 형태(96×96 흑백, 5클래스, 최적화기 가중치 포함)라서 **직접 학습해 저장한 모델로 보이지만 이것은 추정**이다. 누가 어떤 데이터로 학습했는지, 그 데이터의 라이선스는 확인하지 못했다.

**재현 환경 요건(고정 소스 기준).** `requirements.txt`: `tensorflow==2.15.0`, `opencv-python==4.9.0.80`, `cvlib==0.2.7`, `Pillow==10.2.0`, `numpy==1.26.3`. `Dockerfile`은 `python:3.10-slim`(libgl1·libglib2.0-0 등 시스템 라이브러리 필요). 얼굴 검출은 `cvlib`의 OpenCV SSD(`res10_300x300_ssd_iter_140000.caffemodel`, `deploy.prototxt`)이고 cvlib(MIT)의 master 소스는 이를 패키지 데이터로 불러온다(0.2.7 배포본은 열어 보지 않았다: 확인 필요). 전처리: UI가 `getScreenshot()`의 `split(',')[1]`로 접두어를 뗀 순수 base64 JPEG → PIL → numpy 배열(RGB) → `cv.detect_face` → 첫 얼굴 crop → 96×96 resize → `COLOR_BGR2GRAY`(RGB 배열에 BGR 계수를 쓰는 원본의 채널 취급 그대로) → `/255` → `(1,96,96,1)` → `predict` → 각 값 `round(x*100, 2)`. 얼굴이 없거나 오류면 `neutral 100.0`의 기본값. Keras 2.4.0으로 저장한 H5를 TF 2.15(Keras 2.15)로 읽는 호환성은 실행하지 않았다(미검증). Apple Silicon에서 TF 2.15 설치 가능 여부도 확인하지 않았다(미검증). 실추론 시험의 입력으로 쓸 사용 허가된 영상·프레임이 저장소 어디에도 없다(앞선 조사와 같다).

**결정이 필요한 항목(사용자).** 모델의 사용 허가 근거가 문서·이력으로는 확인되지 않는다. 선택지: (A) 원본 모델 사용: 사용자가 이 모델의 소유·사용 권한을 확인하고 학습 데이터의 라이선스 상태를 감수한다고 정하면, 위 환경을 CI(Linux x86_64)에 재현해 같은 입력으로 원본과 점수를 대조한다. (B) 허가된 대체 모델: 라이선스가 명확한 모델을 같은 입출력 계약(96×96×1 → 5클래스)으로 쓰되 점수가 달라져 원본과의 수치 대조는 못 한다. (C) 보류: 지금의 전송·세션·타임라인 저장 경로까지만 두고 감정 값은 비운 채 둔다(현재 상태). A·B는 모두 TensorFlow/OpenCV 전문 런타임을 서버 밖에 두는 의존을 남긴다(작업 지시의 "전문 ML runtime 유지 시 잔여 확장/운영 의존 표시").

## 2026-10-23 `/events/socket`의 `permessage-deflate` 거절 (프로토콜 수준 재현·수정, 실제 브라우저는 미실행)

원래 서버는 `Sec-WebSocket-Extensions` 헤더가 하나라도 있으면 핸드셰이크를 `WIRE.BAD_REQUEST`로 거절했다. 브라우저는 항상 `permessage-deflate`를 제안하므로(Chrome은 `permessage-deflate; client_max_window_bits`) 브라우저의 `/events/socket` 연결이 막힐 수 있다. RFC 6455 9.1절은 서버가 제안된 확장을 하나도 받지 않아도 된다고 한다. 시험 `a_browsers_extension_offer_is_declined_not_refused`(`browser_websocket.rs`)가 Chrome의 헤더 값으로 먼저 FAILED였다: `Http(Response { status: 200, ... "WIRE.BAD_REQUEST" ... "잘못된 WebSocket handshake" })`. 거절 조건을 지운 뒤 통과했고(`browser_websocket` 4 passed, `realtime` 3 passed, `socketio_transport` 5 passed) 101 응답에 `sec-websocket-extensions`가 없다. **실제 브라우저 실행은 하지 않았다**: 이 세션에는 브라우저 도구가 없고 지정된 사전 점검 없이 로컬 브라우저를 띄우지 않았다. 그래서 "브라우저가 막힌다"는 것은 헤더 값과 RFC로 뒷받침한 추정이고, 수정 뒤 브라우저가 연결된다는 것도 확인하지 못했다.

## 2026-10-23 WBS 7.2.10 조각 1: UI 읽기 data access 어댑터 (구현 조각, 어댑터 단위 시험만, 7.2.10은 진행 중)

**범위.** 결정 [D-P-UI-ADAPTER](../docs/DECISIONS.md). `porting/facereview/ui-overlay/`가 고정 UI(`2e7c021`)의 `src/api/youtube.ts`가 하는 읽기 9개(`getVideoList`, `searchVideos`, `getAllVideo`, `getPersonalRecommendedVideo`, `getVideoDetail`, `getRecentVideo`, `getEmotionSummary`, `getRelatedVideo`, `getBookmarkVideos`)를 같은 이름·인자·반환으로 SDK 확장 호출에 연결한다. 읽기 확장이 JSON 텍스트로 돌려주는 목록·객체를 파싱해 원본 응답 모양으로 돌려주고, 시청 상세는 읽은 뒤 `Video.view`를 적용해 원본의 조회수 증가를 대신하며, 감정 요약은 쓰기 확장이라 호출마다 새 key를 쓴다. **이 조각은 어댑터만 시험했다. UI를 빌드하거나 실제 SDK·서버·브라우저로 실행하지 않았다.**

**실행한 검증.** `node --test porting/facereview/ui-overlay/tests/readAdapter.test.ts` → `tests 12 / pass 12 / fail 0`(가짜 SDK 클라이언트): 인자 매핑(검색의 빈 `emotions`, 원본 기본값 page 1·size 10, 최근 시청의 emotion `all`), 응답 파싱(`categories`·`videos`·`timeline_data`·`body`), 시청 상세가 원본 골든 사례(`slice13f-golden.json`의 `expected`)와 같은 필드에 타임라인만 파싱된 값, 읽은 뒤 한 번만 `Video.view`를 적용(실패해도 경고만), 요약 호출마다 다른 key와 거절 시 와이어 코드를 가진 오류, 텍스트가 없거나 JSON이 아니면 `CLIENT.PROTOCOL_ERROR`, 클라이언트 오류의 그대로 전달. 시험을 구현 뒤에 썼으므로 변이로 확인했다: 최근 시청의 기본 size 변경, 타임라인 파싱 제거, 검색의 `emotions` 변경 세 가지는 각각 시험 1개를 FAILED로 만들었다(변이 복원 뒤 12 pass). `tools/verify.sh`의 `node --test` 목록에 이 시험을 더했다.

**확인한 것.** 생성 binding(`aip service gen --lang ts`)에 확장 설명자(`Video.homeCategory` 등)와 쓰기 확장 설명자가 있고 SDK `connect` 클라이언트에 `extension`·`writeExtension`·`apply`가 있다. 쓰기 확장 설명자에 클라이언트가 부를 수 있는 `User.startWatching`·`User.recordFrame`도 나온다(결정의 열린 문제 d).

**CI.** 이 조각의 커밋 `bdb8f5abe06509cfecfecd6057e4dd9aa4940e04`는 Verify(run 39806502935)만 failure였고 나머지 7종은 success였다. 원인은 이 조각이 아니다(바뀐 파일에 Rust가 없다): `spikes/spike-v6-transport/tests/bundle_http.rs`의 "숨은 행의 값이 중복 오류에 담기지 않는다" 단정이 오류 본문 전체에서 문자열 `900`을 찾는데, 본문의 무작위 `requestId`(16진수, 그 실행은 `...bc0900d1d7`)에 `900`이 우연히 들어 있었다. 약 0.5%의 확률로 나는 시험의 결함이라 서버가 말하는 `code`와 `msg`만 검사하도록 고쳤다(수정 뒤 로컬 `1 passed`). 수정 커밋 `27806e44b18ac99cfdc6c3d754b328d7251066b5`의 CI는 Linux cgroup v2·Object storage·Linux worker jobs·App packages·External brokers·Verify(run 39808931788)가 success다(Database contracts·Deployment checks는 경로 조건으로 실행되지 않았다). Verify 로그에서 `bundle_http`가 통과하고 어댑터 시험 12개(예: `home category ...`, `watch detail equals the original ...`)가 Linux runner에서 실행된 것을 확인했다.

**열린 문제(결정 필요, 상세는 결정 기록).** (a) 비로그인 방문자는 확장 호출이 서명된 세션을 요구해서 홈 목록이 서비스되지 않는다. 이 문제는 7.2.3 때부터 알려진 코어 의존(extensions README 3번)이고 화면에서는 방문자의 랜딩 화면이 막히는 것으로 나타난다. (b) 좋아요·북마크 토글. (c) 인증·재발급 인터셉터. (d) 내부 전용 확장 표시가 없다. **미검증:** `aipClient.ts`(UI 인증 저장소·생성 binding·`@aip/sdk` 필요), 실제 서버에 대한 호출, UI 빌드와 브라우저, 각 응답이 화면 타입과 필드별로 같은지(원본 오라클과의 서버 쪽 일치만 있다).

## 2026-10-23 WBS 7.2.10 조각 2: 인증·쓰기 어댑터와 pinned UI 빌드 (구현 조각, 7.2.10 진행 중)

**범위.** 결정 [D-P-UI-ADAPTER](../docs/DECISIONS.md)의 이미 정해진 범위에서 `auth.ts`, `authAdapter.ts`, `writeAdapter.ts`와 SDK 연결을 overlay에 추가하고, 원본 dirty checkout은 수정하지 않았다. `authTransport`는 UI가 쓰던 `/auth/check-email`, `/auth/login`, `/auth/signup`, `/auth/logout`, `/auth/reissue` 계약을 호출한다. 보호된 SDK 요청에서 첫 401에만 refresh를 시도하고 동시 401은 동일 refresh Promise를 공유하며 각각 원 요청을 한 번만 재시도한다. 재발급 실패는 기존 unauthorized callback을 부르고 원래 실패를 던진다. token은 상태 store로만 전달하며 로그·오류 문자열에 출력하지 않는다. `writeAdapter`는 현재 본인 상태에 따라 명시적 `VideoBookmark.create/remove`, `VideoLike.create/remove`를 고르고 `Comment.create/edit/delete`, `VideoRequest.recommend`에 연결한다. 미이식 API는 fallback stub이 `PORT.NOT_PORTED`로 거절해 이전 REST backend로 돌아가지 않는다.

**테스트 우선 실행.** `authAdapter.test.ts`는 구현 전에 3개 red(인증 호출/동시 401/refresh 실패), 1개 통과 상태를 확인했고, `writeAdapter.test.ts`는 미구현 오류로 4개 red였다. 구현 후 `node --test porting/facereview/ui-overlay/tests/*.test.ts` 결과는 **20 pass, 0 fail**(read 12, auth 4, write 4). 최종 재실행 원문 로그: `/Users/winterholic/.ttakkari/data/outbox/5dd19407-d55f-430f-90fe-aecaf49d515f/aip-7.2.10-node-tests-final-2.log`; 앞선 실행과 red 증거는 같은 폴더의 `aip-7.2.10-overlay-node-tests-final.log`, `aip-7.2.10-auth-red-2.log`, `aip-7.2.10-write-red.log`다.

**생성 SDK와 UI build.** `aip service gen --lang ts --sdk-import @aip/sdk`가 실제 타입 binding을 생성했다(fingerprint `520b4e41bd3f6a3d8b3c13db10e5f4420a08e2e8a173672d31a444540391421f`, `idWire=decimal-string-v13`). 실제 생성 결과를 `@aip/sdk@0.1.0` tarball로 pack해 pinned checkout에 설치했다. 원본 UI 저장소는 revision `2e7c021fa5abebd72278d841b8066a33d45cfbb3`에서 dirty였으며, `/private/tmp/aip-ui-7.2.10-20261023c`의 clean pinned checkout에만 overlay script를 적용했다. 같은 임시 checkout에서 `node .yarn/releases/yarn-4.12.0.cjs tsc --noEmit` 통과, `vite build` 종료 0(최종 재실행 `✓ built in 1.11s`). 전체 `yarn build`는 tsc/Vite client build 뒤 기존 prerender가 5,000개 영상 API/thumbnail 경로로 진입해 도중에 중단(exit 130)했다. 그래서 **TypeScript 및 실제 client bundle build 통과**, 전체 정적 prerender는 미완료로 구분한다. 최종 bundle build 원문은 outbox의 `aip-7.2.10-ui-build-final-2.log`; 생성 SDK·첫 build 실행 로그는 `aip-7.2.10-sdk-package-build.log`, `aip-7.2.10-ui-typecheck-vite.log`, `aip-7.2.10-ui-build-final.log`다.

**브라우저.** configured Ttakkari browser에서 `http://192.168.0.186:4173/`가 `/main`으로 이동했고 FaceReview title을 보였다. 접근성 snapshot은 Logo와 main 영역 셸을 보여 주며 콘솔 메시지 0개, 오류 0개다. 이 preview는 서비스 API/인증을 제공하지 않아 화면 데이터·로그인·쓰기는 검증하지 않았다. 첫 `localhost` 접근은 다른 앱으로 라우팅된 것을 확인하고 올바른 pinned preview 주소로 다시 열었다. screenshot은 `/Users/winterholic/.ttakkari/data/outbox/5dd19407-d55f-430f-90fe-aecaf49d515f/browser/page-2026-10-22T16-30-44-396Z.png`다.

**공개 경로와 권한 판정.** 기존 D-P-WATCH-SESSION은 `VideoViewLog.create`를 `active(actor) and user = actor`로 명시 개방했다. 그러므로 본인 행을 직접 만드는 일은 현재 인증/행 권한 계약의 우회가 아니지만, 본인 시청 이력과 주간 활동 집계에 영향을 주는 이미 기록된 권한 한계다. `User.startWatching`·`User.recordFrame`은 generated write-extension contract와 HTTP `/apply` 모두에서 호출 가능한 것으로 선언돼 있다. 코드상 `startWatching`은 actor 본인 행을 생성하고 `recordFrame`은 자기 세션 소유 확인 뒤 server-staged 점수만 `getdel`한다. SDK에서 감추는 것만으로는 보호되지 않는다. D-P-WATCH-FRAMES의 server-only call intent에는 미달하지만, 여기서 임의 점수 입력 또는 다른 사용자 세션 접근을 확인한 것은 아니다.

**실행 음성 검사 상태.** 실행을 위해 다른 작업의 `aip-p730-*` fixture를 건드리지 않고 이 Run 소유의 MySQL 8.4·MongoDB 8.0·Redis 7.4를 별도 포트에 준비했다. `cargo test -p aip-cli --features mysql --test mysql_facereview_watch --locked -- --ignored --test-threads=1`는 실제 test binary까지 빌드했지만, `aip service serve` startup stage가 `SERVER_CONFIG`, `io other error`와 함께 종료돼 HTTP `/apply` negative/positive 판정까지 도달하지 못했다. 공개 호출 경로라는 결론은 `.aip` declaration → HTTP router 계약 → 실제 callsite → generated binding 대조에 따른 **코드 기반 판정**이다. **이 Run의 Mac workspace 실행에서는 live service/API 경로 미검증**이고 배포 backend나 CI의 현재 실행 결과로 일반화하지 않는다. 원문 로그: `/Users/winterholic/.ttakkari/data/outbox/5dd19407-d55f-430f-90fe-aecaf49d515f/aip-7.2.10-live-watch-authz-retry-2.log`(exit 101; failure stage: server startup readiness, 하위 실패 원인 미확인).

**당시 기능 제한과 후속(현재 상태는 아래 2026-10-23 조각 3).** 이 조각에서는 server-only 표식이 아직 없어 공개 경계가 미완이었다. 이후 AIP extension 내부 표식은 아래 조각 3에서 추가한다. `model.h5` 권리·출처 확인이나 모델 실행은 하지 않았다. 비로그인 접근 정책을 새로 정하지 않았다. mypage/admin의 나머지 미이식 API, API 응답의 원본 UI 필드 동등성, 로그인 완료 journey, 전체 prerender 및 CI는 미검증이다. 진행 기준은 “독립 인증·쓰기·build 조각 확인”이지 WBS 7.2.10 전체 완료가 아니다.

## 2026-10-23 WBS 7.2.6/7.2.10 조각 3: internal write extension 경계 (동일 SHA CI 통과)

**결정과 범위.** [D-P-INTERNAL-EXTENSION](../docs/DECISIONS.md)은 기존 [D-P-INTERNAL-APPLY](../docs/DECISIONS.md)의 최소 opt-in 노출 표식을 write extension에 적용한다. `facereview.aip`의 `User.startWatching`과 `User.recordFrame`만 `internal`로 표시했다. 표식이 빠진 기존 공개 확장은 실행 사실·SDK 타입·공개 fingerprint가 변하지 않도록 기본 호환으로 두고, `internal`이 참인 경우에만 정식 facts에 기록한다. `VideoViewLog.create`의 공개 명시 허용은 유지했다.

**언어·SDK 및 경계 검사.** parser·H-map·semantic facts·정의 diagnostics에 표식을 연결했다. `spike-v5-sdk`는 internal 확장을 TS/Python 생성 결과와 계약 fingerprint에서 제외한다. `/apply`의 direct write-extension route는 internal marker 확인 뒤 `WIRE.NOT_EXPOSED`로 거부하고, caller JSON의 추가 `internal` 속성은 strict request shape에서 거부한다. `/extension`은 이 write 확장을 read invocation으로 공개하지 않는다. worker proxy가 extension을 직접 부르는 호출 구조는 없고 `ctx.data.apply`는 기존 `access` grant와 `D-P-INTERNAL-APPLY` 규칙을 유지한다. `startWatching`의 trusted Socket.IO 경로는 같은 `run_relational_write_extension` 실행기를 계속 사용하며 세션 검증·principal-derived user·슬롯·deadline·row policy·saga/fence/commit 경계를 건너뛰지 않는다. `recordFrame`은 staged-score 소켓 호출부가 아직 없으므로 이 조각에서는 공개 거부를 적용한다.

**실행 증거.** Mac Run에서 `cargo test -p spike-v1-fixture --test v1 --offline` → 9 pass, `cargo test -p spike-v5-sdk --test write_extensions --offline` → 2 pass, `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test write_extensions_http --no-run --offline`와 `cargo test -p aip-cli --features mysql --test mysql_facereview_watch --locked --no-run --offline` → 두 통합 시험 컴파일, `cargo fmt --all -- --check`와 `git diff --check` → pass. Linux CI Verify `39824863529`, exact head SHA `71b0bbdb77fa6db2c173691f953a0f1797be66ea`, 전체 `tools/verify.sh` 단계와 live HTTP→Node/Python→MySQL E2E 단계 모두 success. 해당 Linux job에서 parser/H-map·TS/Python SDK 계약 검사가 통과했고 `write_extensions_http`의 `internal_write_extensions_are_not_http_callable_and_payload_flags_do_not_grant_trust`도 1 pass: `/apply`, `/extension` 거부 및 caller flag 위조가 허용되지 않았다. `scripts/verify-mysql-worker-e2e.sh run-ci`가 실제 `mysql_facereview_watch` 시험을 실행해 `facereview_watch_detail_matches_the_original_service_through_the_real_server` 1 pass를 기록했다. 이 시험은 실제 MySQL write/socket 정상 경로와 MongoDB·Redis 효과를 함께 확인한다. 원문 CI 로그는 새 outbox의 `aip-internal-extension-ci-verify-39824863529.log`이며, `9adc187`의 최초 실패 로그도 `aip-internal-extension-ci-verify-39822711060.log`에 보존했다. 최초 실패는 생성 syntax markdown golden 불일치였다. syntax card에서 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test diagnostics spec_definition_syntax_is_generated_from_the_card --offline`로 재생성한 뒤 diagnostics 21 pass, 최종 동일 SHA CI는 success.

**startup 원인 조사와 한계.** 이전 원시 live log는 `cargo test -p aip-cli --features mysql --test mysql_facereview_watch --locked -- --ignored --test-threads=1`의 test binary가 `aip service serve` 시작 단계에서 exit 101, `SERVER_CONFIG` / `io other error`를 반환한 것만 보존한다. 소스 경로는 `mysql_service::serve` → `listen_relational_with_worker_writes` → `WorkerLaunchConfig::probe`다. service wrapper는 `ErrorKind::Other`의 내부 stderr를 generic `SERVER_CONFIG` 메시지로 감춘다. 같은 Codex Mac test environment에서 Python worker probe를 분리 재현한 결과는 `exit status: 71`, stderr `sandbox-exec: sandbox_apply: Operation not permitted`였다. 이는 반복 실패의 원인을 해당 worker startup probe로 좁히는 실행 근거이며, Mac nested worker sandbox 범위다. 정확한 이전 live startup의 stderr는 wrapper가 버려 원본 run으로부터 복원할 수 없다. 운영 backend의 장애로 일반화하지 않고, 알려진 sandbox 거부를 우회하거나 같은 startup 명령으로 재시도하지 않았다.

**미검증과 한계.** PostgreSQL은 CI의 전체 검증 harness에 포함되지만 `User.startWatching`/`recordFrame`이 선언된 FaceReview 런타임은 MySQL adapter다. 따라서 PostgreSQL에서 해당 trusted extension의 실제 저장 효과를 검증했다고 주장하지 않는다. `recordFrame`의 future trusted caller는 아직 없고, 공개 거부와 SDK 제거만 검증했다. 실제 감정 분석, 브라우저 로그인·저장 journey, 정적 prerender 및 model permission은 미실행이다. 기존 Mac Run의 MySQL live startup은 Codex nested worker sandbox의 Python probe가 `sandbox-exec: sandbox_apply: Operation not permitted`(exit 71)로 거부돼 서비스 startup에서 막혔다. 이는 해당 실행 환경의 한계이며 deployed backend 장애 근거가 아니다. 최종 Linux CI가 실제 MySQL·MongoDB·Redis test를 완료했으므로 이번 기능 경계/정상 경로 판정은 동일 SHA CI 증거를 따른다.

## 2026-10-22 WBS 7.2.10 사전 조사: 원본 UI의 data access 표면 (조사만, 구현 없음)

기준은 UI revision `2e7c021fa5abebd72278d841b8066a33d45cfbb3`(`projects/facereview/facereview-front-joowon`의 pinned commit, 작업 트리는 읽지 않았다)의 `src/api/{index,auth,mypage,request,watch,youtube}.ts`와 `src/socket/index.ts`다. 서버 호출은 이 여섯 파일과 소켓 한 곳에 모여 있고 axios 인스턴스(`baseURL: '/api'`, `withCredentials`, 401이면 `/v2/auth/reissue`로 한 번 재시도하는 인터셉터)를 공유한다. 서버 호출은 30곳(소켓 제외), 고유 경로는 25개다.

| UI 호출 (`/api/v2/…`) | AIP 쪽 대응(정의·조각) | 상태 |
|---|---|---|
| `auth/check-email`, `login`, `signup`, `reissue`, `logout` | 서비스 `credentials`의 `/auth/*` (7.2.2) | 서버 구현·시험 있음. 경로·쿠키·응답 모양 대조는 화면 쪽 |
| `auth/me`, `auth/tutorial`(PATCH), `mypage/profile`(PATCH), `mypage/withdraw` | `/auth/me`, `User.completeTutorial`, `User.updateProfile`, `User.withdraw` (7.2.8) | 서버 있음 |
| `mypage/email/verification·verify`, `mypage/password/verification`, `mypage/password` | `credentials.mail`의 `/auth/email/*`·`/auth/password/*` (D-P-CREDMAIL) | 서버 있음, 실제 메일 provider는 미실행 |
| `home/category`, `home/search`, `home/video/all`, `home/personalized`, `home/bookmark`(GET) | 읽기 확장 `Video.homeCategory`·`homeSearch`·`homeVideoAll`·`homePersonalized`·`homeBookmark` (7.2.3) | 서버 있음 |
| `watch/video`, `watch/recommended` | `Video.watchDetail`(+ `Video.view`), `Video.watchRecommended` (7.2.5) | 서버 있음 |
| `watch/comments` GET·POST·PATCH·DELETE | `Comment` read·create·`edit`·`delete` (7.2.4) | 서버 있음 |
| `watch/like` POST(좋아요·취소가 같은 POST), `home/bookmark` POST(토글) | `VideoLike`·`VideoBookmark`의 create·remove (7.2.4, 토글 대신 명시적 생성·삭제) | 서버 있음. 화면이 현재 상태를 알고 create와 remove를 골라야 한다 |
| `home/video/recommend` | `VideoRequest.recommend` (7.2.9) | 서버 있음 |
| `mypage/videos/recent`, `mypage/emotion/summary` | `User.myRecentVideos`, `User.myEmotionSummary` (7.2.8) | 서버 있음 |
| Socket.IO `watch_frame` (`transports:['websocket']`, `auth:{token}`) | Socket.IO 호환 전송(7.2.6 조각 1) | 전송 경계만 있다. 분석·저장·ML 없음 |

**알려진 사실과 열린 문제.** (1) 이 표의 대응은 정의 이름과 각 조각의 기록을 읽어 만든 것이고, 화면이 쓰는 응답 모양(예: 원본의 `/v2/…` JSON)과 각 확장 출력의 필드별 일치는 이 조사에서 다시 확인하지 않았다. 각 조각의 오라클 대조가 서버 쪽 일치를 말할 뿐 화면 일치는 아니다. (2) 서버 쪽 경로 규약이 다르다: 원본은 `/api/v2/...` REST, AIP는 `/auth/*`·`/extension`·`/apply` 등이라서 UI의 data access 여섯 파일을 SDK 호출로 바꾸고 응답을 화면의 타입(`VideoDataType` 등)으로 맞추는 어댑터가 필요하다. 인터셉터의 401 재발급 흐름도 AIP의 접근 토큰 300초와 단회용 refresh 규칙에 맞게 다시 확인해야 한다. (3) 고정 UI에 포함되지 않은 호출(마이페이지 하이라이트·캘린더·모먼트·DNA, 관리자 화면)은 이 revision의 `src/api`에 없다. 관리자 UI는 별도 저장소(`new-facereview-admin-front`)이며 이 조사 범위 밖이다. (4) `addLike`·`cancelLike`는 같은 `POST watch/like`이고 코드 주석이 "backend toggles like state per video"라고 적으며, `toggleBookmark`도 `POST home/bookmark` 하나다. 즉 UI는 현재 상태를 보내지 않고 서버가 토글한다. AIP는 명시적 create·remove(7.2.4)라서 화면이 현재 상태(`user_is_liked`·`is_bookmarked`)를 알고 둘 중 하나를 골라 부르는 변환이 어댑터에 들어간다. 연속 클릭과 경쟁에서 원본 토글과 의미가 달라질 수 있어 화면 시험이 필요하다. (5) 최종 gate(원본 backend를 끈 브라우저 E2E)는 7.2.6의 실제 ML 흐름이 끝나기 전에는 닫을 수 없다. 다음 조각은 UI 작업 위치(원본 React 사본을 어디에 둘지)와 SDK 생성 바인딩 산출 방법을 정하는 것이다.

## 2026-10-22 WBS 7.2.8 조각 C: 마이페이지 감정 읽기 5개 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.8은 진행 중)

**범위.** 원본 `MypageService`의 `get_recent_videos`·`get_highlight`·`get_emotion_calendar`·`get_moments`·`get_emotion_dna`를 읽기 확장 `User.myRecentVideos`·`myHighlight`·`myEmotionCalendar`·`myMoments`·`myEmotionDna`로 옮겼다. 결정과 대안은 [DECISIONS](../docs/DECISIONS.md)의 D-P-USERSTATS다. Mongo capability에 파생 컬렉션 `user_watch_stats`를 더했다: 호출자 본인의 `youtube_watching_data`만 읽고 명령 6개(`sessions`·`compress`·`highlight`·`calendar`·`moments`·`dna`)가 큰 프레임 타임라인을 화면에 필요한 수치로 줄여 돌려준다. 화면 규칙(반올림·묶기·DNA 유형·특성·재미 문구)은 원본 코드를 복사한 확장이 맡는다. `GET mypage/emotion/summary`는 아직 옮기지 않았다.

**원본과의 대조(`reference/slice14e_oracle.py`).** 같은 MySQL·MongoDB에서 원본 서비스와 실제 `aip service serve`를 나란히 돌렸다. 생성한 세션 15개(사전·배열 타임라인, 60프레임 창을 넘는 130프레임 세션, 해석 못 하는 키, 빈 타임라인, 삭제된·없는 영상의 세션, 자정·밀리초 경계, 80~85 구간의 단독 피크)에서 66개 사례가 모두 같았다(`the my page screens matched the original in 66 cases`): 최근 시청 18(감정 필터·쪽·크기), 하이라이트 3, 캘린더 21(연·월·빈 달), 모먼트 21, DNA 3(`generated_at` 제외), 게스트 거절. **변이 확인:** 모먼트 창 크기 60을 20으로 바꾸면 모먼트 11건이, 피크 임계값 80을 85로 바꾸면 4건이 실패했고 복원했다(처음 임계값 변이는 데이터에 단독 피크가 없어 못 잡아서 그 세션을 더했다). CI 재생: `mysql_facereview_trending_job`이 같은 세션(`slice14e-sessions.json`)을 넣고 기록된 66개 답(`slice14e-golden.json`)을 `/extension`으로 재생해 `1 passed`(약 20초).

**알려진 차이와 한계.** (1) 삭제된 영상은 행 정책이 숨겨서 원본이 세던 모먼트·DNA 장르 수에서 빠진다. (2) DNA를 호출마다 계산한다(원본은 `user_emotion_dna`에 7일 캐시하고 새 시청 5건 미만이면 캐시를 돌려줌). 시험은 고정 데이터의 캐시 행을 지우고 비교했다. (3) DNA의 장르 수는 서로 다른 영상 약 3000개까지 읽는다(ctx 64회). 넘으면 틀린 값을 내지 않고 실패한다. (4) 선택 입력(`month`)은 null로 보내야 한다. (5) `emotion/summary`(`user_emotion_summary`를 낙관적 잠금으로 갱신하는 증분 집계)는 쓰기가 필요해 다음 조각이다. (6) 파이썬 `int()`가 받는 숫자 구분 밑줄 키는 해석하지 못한 키로 센다.

## 2026-10-22 WBS 7.2.9 요청 승인 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.9는 진행 중)

**범위.** 원본 `AdminService.approve_video_request`(영상 생성, `video_distribution`·`video_timeline_emotion_count` 초기 문서, 요청 `ACCEPTED`를 한 union 트랜잭션에서)를 durable job `approveVideoRequest`로 옮겼다. 쓰기 확장은 전이만 부를 수 있어서 7.2.7의 job `create`·Mongo 쓰기를 쓰는 job으로 했다(관리자는 새 영상 id를 `/jobs/result`에서 읽는다). `VideoRequest`에 전이 `accept`(관리자, `PENDING`에서, 요청의 `youtube_url`을 가진 `Video`가 있을 때만)를 더해서 클라이언트가 영상 등록 없이 요청을 승인 상태로 만들 수 없다. 거절(`reject`)은 이미 있었다. `save_video`가 새 영상의 id를 돌려주도록 바꿨다(이미 있으면 None).

**원본과의 대조(`reference/slice14b_oracle.py`).** 같은 시작 데이터에서 원본 `AdminService.approve_video_request`와 AIP job을 각각 돌렸다. 영상 행(`reqvideo001`, 관리자가 고른 제목·채널·길이 321·장르 vlog, 조회수 0, 활성), 두 Mongo 초기 문서, 요청 상태(`ACCEPTED`, 코멘트 없음)가 모두 같았다(`PASS approve video/documents/request status`, `the trending, the category fill and the approval jobs matched the original`).

**실서버 시험(`mysql_facereview_trending_job`의 새 단계, `1 passed`, 약 16초).** 회원은 승인 job을 시작할 수 없다. 관리자의 승인은 영상을 만들고(관리자가 정한 값, 조회수 0, 활성) 문서 둘을 만들고 요청을 `ACCEPTED`로 하며 새 영상 id가 job 결과에 있다. 같은 key는 같은 run이다. 이미 처리된 요청, 이미 같은 `youtube_url`의 영상이 있는 요청, 없는 요청은 `FAILED`이고 요청 상태·영상·문서가 그대로다. 클라이언트가 `VideoRequest.accept`를 직접 부르면(영상 등록 없이) 거절되고 요청이 `PENDING`으로 남는다. **변이 확인:** 전이의 영상 존재 조건을 빼면 직접 승인이 통과해 실패했고, job에서 `accept` 호출을 빼면 요청이 `PENDING`으로 남아 실패했다. 둘 다 복원했다.

**알려진 한계.** (1) 승인은 비동기 job이다(원본은 응답으로 `video_id`를 줌). (2) job에는 saga가 없다: 영상을 만든 뒤 Mongo나 `accept`가 실패하면 되돌리지 않고 재시도가 장부를 재생해 끝낸다. 재시도 중 사람이 같은 요청을 거절하면 영상은 남고 요청은 `REJECTED`다. (3) 중간 실패 뒤 재시도 수렴은 자동 시험으로 보이지 않았다(장부 재생 자체는 7.2.7 시험이 보인다). (4) 요청 생성 `POST home/video/recommend`는 위 절에서 옮겼고, dashboard, 시스템 상태, 더미 데이터는 7.2.9에 남아 있다.

## 2026-10-22 WBS 7.2.8 조각 B: 이메일 인증번호와 비밀번호 재설정 (구현 조각, 로컬 실DB·실서버·MariaDB 저장소 통과, 7.2.8은 진행 중)

**범위.** 원본 마이페이지의 `email/verification`, `email/verify`, `password/verification`, `password`(PATCH)를 서비스의 비밀번호 로그인 곁에 옮겼다. 결정과 대안은 [DECISIONS](../docs/DECISIONS.md)의 D-P-CREDMAIL이다. 서버 설정 `credentials.mail`(`integration`, `verified_field`, 선택 `updated_field`·`code_ttl_secs` 300·`reset_ttl_secs` 600·`max_attempts` 5)을 켜면 네 경로가 `POST /auth/email/verification`·`/auth/email/verify`·`/auth/password/verification`·`/auth/password`로 생기고 모두 Bearer 접근 토큰이 필요하다(전송 계층이 토큰을 그대로 넘기고 서비스가 다른 경로와 같은 검증기로 확인한다). 원본이 Redis에 두던 번호·토큰은 migration 계정의 `adopt-external --apply`가 만드는 `aip_auth_challenges` 표(주체·용도별 한 행, SHA-256 digest, 시도 횟수, 만료)에 둔다. 번호는 설정된 POST 연동(`{to, template, code, expires_in_secs}`)으로 보낸다.

**실서버 시험(`mysql_facereview_auth`, `1 passed`, 약 14초, 로컬 스탠드인 메일 서버).** 토큰이 없거나 변조된 네 경로는 모두 401이고 메일은 나가지 않았다. 주소를 요청 값으로 주면 422다. 번호 요청은 계정 자신의 주소로 한 통만 나가고(`template`·`expires_in_secs 300`), 표에는 digest만 있다. 틀린 번호는 400이며 횟수가 1이 되고, 형식이 틀린 번호는 422, 다른 계정의 세션은 그 번호를 쓰지 못한다. 맞는 번호는 `is_verify_email_done`을 1로 하고 번호를 소모해 재사용은 400이다. 틀린 번호 5번이면 맞는 번호도 429이고 비밀번호 경로에서도 같다. 새로 요청하면 교체되어 횟수가 0으로 돌아온다. 만료된 번호는 없는 것과 같고 행이 정리된다. 제공자가 500이면 503이고 번호가 남지 않는다. 비밀번호: 번호를 재설정 토큰(64자 16진)으로 바꾸면 번호는 소모되고 토큰은 digest로만 저장된다. 너무 짧은 새 비밀번호(422)나 다른 계정의 토큰 사용(400)은 토큰을 쓰지 않으며, 맞는 토큰이면 해시가 현재 기본 scrypt로 바뀌고 옛 비밀번호는 죽고 새 비밀번호로 로그인된다. 변경한 세션은 살아 있고 다른 로그인은 모두 끝나며 토큰은 한 번만 쓰인다. **변이 확인:** 시도 횟수 검사를 끄자 잠금 단언이 실패했고(맞는 번호가 200), 다른 세션 폐기를 지우자 `every other login ended`가 실패했다. 둘 다 복원했다. 저장소 단위 시험(`crates/aip-migrate/tests/mysql_auth_sessions.rs`의 새 1건): digest 길이 32바이트, 용도·주체 분리, 시도 제한과 교체, DB 시계 만료, 열 개의 동시 오답이 3번까지만 `Wrong`(제한 3), `revoke_others`가 MySQL 8.4와 MariaDB 11.4 양쪽에서 통과했다.

**알려진 차이와 한계.** (1) 원본의 6자리 번호는 평문으로 Redis에 5분 있었고 시도 제한이 없었다. 이 포팅은 digest와 시도 제한을 두었다(의도한 차이). 원본은 비밀번호를 바꿔도 다른 기기의 세션을 살려 두었으나 여기서는 끝낸다. (2) 번호 재요청의 빈도 제한은 없다(proxy 또는 `rate_limit`의 몫이며 7.2.2의 로그인 시도 제한과 같은 처리). (3) 메일 본문(HTML 템플릿)은 제공자 쪽이 `template` 이름으로 만든다. 원본의 SMTP 직접 전송과 HTML 본문은 옮기지 않았다. (4) 실제 메일 제공자 live smoke는 미실행이다. (5) 원본 재설정 토큰은 이메일에 묶였고 여기서는 계정(subject)에 묶인다. (6) 브라우저 쿠키·교차 사이트 실행은 7.2.10이다. (7) 이 증분은 마이페이지의 Mongo 감정 통계 읽기(`videos/recent`, `emotion/summary`, `highlight`, `emotion/calendar`, `moments`, `emotion-dna`)를 다루지 않았다.

## 2026-10-22 WBS 7.2.8 조각 A: 약한 해시 재해시, 튜토리얼, 프로필, 선호 장르, 탈퇴 세션 종료 (구현 조각, 로컬 실DB·실서버 통과, 7.2.8은 진행 중)

**범위.** 7.2.2가 넘긴 인증 잔여와 마이페이지의 관계형 부분이다. (1) 로그인 성공 뒤 저장된 해시가 현재 기본(`scrypt:32768:8:1`)보다 약하면 서버가 같은 비밀번호로 새 해시를 만들어 바꾼다. 확장의 `verify_password`가 `rehash` 판정을 돌려주고, 갱신은 로그인 트랜잭션 안에서 "검사한 그 해시일 때만"(`WHERE password = <검사한 해시>`) 쓴다. 틀린 비밀번호는 아무것도 쓰지 않고 `updated_at`도 건드리지 않는다. 원본은 재해시를 하지 않으므로 의도한 보강이다. (2) 정식 정의에 `User.completeTutorial`(원본 `complete_tutorial`: 두 번째 호출은 행을 그대로 둠), `User.updateProfile`(`patch name, profile_image_id`: 보낸 값만 바꿈), `UserFavoriteGenre`의 `create`와 `remove`를 더했다. 선호 장르 교체는 한 묶음(`atomic`)에서 기존 행 `remove` 뒤 새 행 `create`다. (3) `/auth/me`는 호출자 본인의 `User` 읽기와 `UserFavoriteGenre` 읽기로 구성한다(별도 endpoint 없음). (4) 탈퇴는 이미 있던 `User.withdraw` 전이이고, 이번에 세션 종료를 시험으로 확인했다.

**플랫폼 수정.** `repeat unchanged` 판정이 `to`의 모든 대입을 현재 행과 비교했는데, `updated_at = now`는 어떤 행과도 같지 않아서 이미 목표 상태인 행도 매번 다시 쓰였다(시험이 찾음: 튜토리얼 반복이 `updated_at`을 바꿈). 시계를 대입하는 필드를 판정에서 뺐다(MySQL `mysql_write.rs`, PostgreSQL `spike-v3-write`). 원본 SQLAlchemy는 값이 그대로면 UPDATE도 `onupdate`도 없으므로 원본에 더 가깝고, 관리자 댓글 삭제 `Comment.remove`·`User.deactivate`의 반복도 같은 영향을 받는다. 기존 시험(`mysql_facereview_reads`·`mysql_facereview_admin`·`mysql_external_effects`·`mysql_adoption_flow`·`service_flow`·`http_blackbox`, `spike-v3-write` 전체)이 그대로 통과했다.

**실서버 시험(`mysql_facereview_auth`, `1 passed`, 약 8초).** (a) Werkzeug `pbkdf2:sha1:1000` 해시 계정: 틀린 비밀번호는 해시를 그대로 두고, 맞는 비밀번호로 로그인하면 `scrypt:32768:8:1$...`로 바뀌고 `updated_at`은 그대로이며, 다시 로그인하면 되고 현재 해시는 바뀌지 않는다. 원본이 만든 기본 scrypt 해시도 그대로다. (b) 가입한 회원의 읽기가 본인 행(이메일·이름·역할·튜토리얼·이메일 인증 여부)과 선호 장르 2개를 준다. (c) 튜토리얼: 남의 행은 거절, 본인은 완료, 반복은 행(`updated_at` 포함)을 그대로 둔다. (d) 프로필: 이름만, 이미지 ID만 보내면 보낸 값만 바뀌고, 남의 행·`role` 값은 거절된다. 선호 장르 교체 묶음이 성공하고, 남의 장르 생성과 이미 있는 장르 생성은 거절되며 거절된 묶음은 아무것도 바꾸지 않는다. (e) 탈퇴: 행은 `is_deleted`가 되고 선호 장르가 지워지며 탈퇴한 계정은 자기 행도 읽지 못한다. 첫 번째 기기의 reissue는 `account_inactive`로 모든 세션을 끝내고 두 번째 기기는 `session_revoked`이며 활성 세션이 0이다. 접근 토큰도 죽고 로그인도 401이다. **변이 확인:** 재해시를 끄자 해시가 그대로여서 시험이 실패했고 복원했다. `now` 판정 수정 전에는 튜토리얼 반복 단언이 `1 2026-10-22 01:16:02` 대 `1 2026-10-02 00:00:00`으로 실패했다.

**알려진 차이와 한계.** (1) 원본 프로필 이름은 2~10자, 이 포팅의 `User.name`은 열 규칙인 1~50자다(가입은 서버 설정이 2~10자를 지킨다). 프로필에서 2~10자를 강제하려면 정의에 필드 단위 검사가 필요하다. (2) 원본 스키마의 "선호 장르는 보낼 경우 최소 1개"를 서버가 강제하지 못한다: 묶음 안의 `create` 개수를 서버가 세지 않아 클라이언트(SDK)가 지켜야 한다. (3) 값이 하나도 없는 프로필 수정(원본은 성공하며 아무것도 하지 않음)은 `values`가 비어서 거절되므로 클라이언트가 호출하지 않는다. (4) 탈퇴 직후 접근 토큰은 다음 reissue 전까지 세션상 살아 있지만 행 정책이 모든 읽기를 막는다. 원본은 presented refresh token만 블랙리스트에 올리고 다른 기기의 토큰은 살려 둔다. (5) 이메일 인증 코드·비밀번호 재설정(Redis와 메일), 마이페이지의 Mongo 감정 통계 화면은 다음 조각이다.

## 2026-10-22 WBS 7.2.7 조각 C: 카테고리 보충(`YoutubeCategoryFillJob`) (구현 조각, 로컬 실DB·실서버·원본 클래스 대조 통과, 7.2.7 완료 대상)

**범위.** 원본 `YoutubeCategoryFillJob`(화·금 03:00 KST)을 durable job `fillCategoryVideos`로 옮겼다. 새 플랫폼 기능은 없다. 조각 B의 쿼리 문자열 연동(`youtube_search` 추가, `youtube_videos`에 `id` 허용)·job `create`·job Mongo 쓰기를 그대로 쓴다. 장르별 활성 영상 수는 `GROUP BY`가 없어 `Video`를 100개씩 읽어 센다.

**원본과의 대조(`reference/slice14b_oracle.py`).** 같은 시작 데이터에서 원본 `execute()`(`requests.get` 대체)와 AIP job을 각각 돌렸다. 새 영상 36행(4개 장르), Mongo 문서 72개, quota 1212·검색 12회가 모두 같았다(`PASS fill rows/fill documents/fill quota and searches/fill saved`, `the trending and the category fill jobs matched the original`). 조각 B의 trending 대조도 같은 실행에서 그대로 통과했다.

**실서버 종단 시험.** `crates/aip-cli/tests/mysql_facereview_category_fill_job.rs`(`1 passed`, 약 28초): 회원은 시작할 수 없고 관리자는 한다. 가장 비어 있는 4개 장르를 원본 순서로, 장르마다 3개 query, 검색 12회, 새 영상 36개(행 36, 분포·타임라인 문서 각 36)와 `{saved 36, searches 12, quota 1212}`, 제공자가 받은 요청 24건(모두 서버의 API key, 호출자가 key를 줄 수 없음). 같은 key는 한 run이고 제공자에게 다시 묻지 않는다. 5번째 검색이 403이면 그 검색이 quota에 들어가고 그 장르의 남은 query(6번째 검색)는 원본처럼 계속 검색하며 다음 장르부터는 검색하지 않는다(`{saved 15, searches 6, quota 1302}`). 응답 없이 끊는 제공자는 run을 다시 시도하게 하고(attempt 2) 이미 온 답은 장부가 재생해 결과가 처음과 같다(요청 25건 = 끊은 1건 + 한 run 24건). **변이 확인:** 장르 사이의 quota 검사를 빼자 `{saved 33, searches 12, quota 1908}` 대 `{saved 15, searches 6, quota 1302}`로 실패했고 복원했다.

**함께 고친 것.** (1) 같은 영상이 두 장르의 검색에 나오면 create key `video:<id>`가 같은데 값(장르)이 달라 두 번째 요청이 거절되고 run이 `WIRE.EXTENSION_ERROR`로 실패했다(디버깅 하니스에서 검색 4회 이상일 때 관찰). key를 `video:<id>:<장르>`로 바꿨다(두 번째 create는 유일 `youtube_url`에 막혀 건너뛴다). (2) CI Verify(39664738540)의 실패 원인은 풀 job 시험의 경합이었다. 캐시 쓰기가 커밋된 직후 run이 SUCCEEDED로 정산되기 전에 개수를 한 번만 세어 0이 나왔다. 정산을 기다리는 폴링으로 고쳤고 로컬에서 62초에 통과했다. (3) 시험의 정렬 단언은 ENUM 열의 `ORDER BY`가 선언 순서라서 `CAST(category AS CHAR)`로 고쳤다.

**알려진 한계.** (1) 한 run이 ctx 호출 약 150회를 쓰므로 서버의 `limits.job_calls`를 1000 이상으로 올려야 한다(기본 64로는 중간에 끝난다). (2) 예약(화·금 03:00)은 조각 A의 풀 job 시험이 보인 같은 경로이고 이 job만의 예약 시험은 하지 않았다. (3) 실제 YouTube credential이 없어서 live smoke는 미실행이다. (4) 프로세스가 커밋 전에 죽는 경우의 복구는 조각 B와 같은 한계를 따른다.

## 2026-10-22 WBS 7.2.7 조각 B: YouTube 인기 영상 수집(쿼리 문자열 연동, job의 행 생성) (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.7은 진행 중)

**범위.** 원본 `YoutubeTrendingJob`(매일 06:00 KST)을 durable job `fetchTrendingVideos`로 옮겼다. 플랫폼에 둘을 더했고 결정은 [DECISIONS](../docs/DECISIONS.md)의 D-P-QUERYEP·D-P-JOBCREATE다. (1) 외부 HTTP 연동의 쿼리 문자열 endpoint(`method` GET, `query_params` 허용 이름, `secret_query_env`로 서버가 붙이는 API key, 지문에 포함, GET이라 응답을 잃은 호출은 UNKNOWN이 아니라 같은 key로 다시 묻는다). (2) job의 `access create.<Resource>`와 `ctx.data.create`(공개 create를 한 연산짜리 atomic 묶음으로 같은 fenced transaction에서 실행). `Video`에 관리자 전용 `expose create`와 필터 `youtube_url.in`을 더했다.

**원본과의 대조(`reference/slice14b_oracle.py`, 같은 격리 MySQL 8.4·인증 있는 MongoDB 7).** 고정 원본(`7948b7e7`)의 `YoutubeTrendingJob`: (a) `_map_youtube_category`·`_classify_category_by_keywords`와 AIP의 포팅이 33개 사례(YouTube 카테고리 11종, 요리 힌트 경계 299·300자, 키워드 동점·없음 등)에서 모두 같았고, isodate 기간 15개 사례(`PT0S`, `P0D`, `PT1H61M`, `P1W`, 빈 문자열, 쓰레기 값 등)도 모두 같았다. (b) 원본의 `execute()`를 `requests.get` 대체(가짜 YouTube, 외부로 나가지 않음)로 돌린 결과와 AIP job을 실제 서버에서 로컬 가짜 YouTube HTTP 서버로 돌린 결과가 같은 데이터에서 새 영상 6개 행(youtube_url·제목·채널·카테고리·길이·조회수·삭제 여부)과 12개 Mongo 문서(`video_distribution`·`video_timeline_emotion_count`, 시각·id 제외)가 같았고 제공자 요청은 3건이었다(`PASS` 5줄, `the trending job matched the original`). 의도한 차이 둘이 출력된다: 제목이 빈 영상은 원본이 빈 제목으로 저장하지만 AIP 필드(`Text(1..255)`)가 거절해 건너뛰고, 빈 채널명은 AIP에서 NULL이다. 원본의 `YoutubeTrendingJob`은 중복 youtube_url 삽입 실패를 로그만 남기고 계속하는데 AIP는 `WIRE.ALREADY_EXISTS`를 건너뛴다(같은 결과).

**실서버 종단 시험.** `crates/aip-cli/tests/mysql_facereview_trending_job.rs`(`1 passed`, 약 13초): 회원은 시작할 수 없고 관리자는 한다. 제공자는 3쪽을 서버의 API key와 허용된 파라미터만으로 받고(`pageToken` 순서, 호출자가 key를 줄 수 없음), 새 영상 6개가 행(원본 카테고리 규칙, 길이 205·3723·600·1530·0·45초, 빈 채널명은 NULL, 조회수 0, 활성)과 두 Mongo 초기 문서(0 분포, `dominant_emotion` null, 빈 `counts`)로 저장된다. 이미 있는 영상·soft delete된 영상(유일 youtube_url이 거절)·반복·제목 없는 영상은 저장하지 않는다(`fetched 9, fresh 8, saved 6`). 같은 key는 한 run이고 제공자에게 다시 묻지 않으며, 새 key는 아무것도 두 번 저장하지 않는다. 제공자의 403은 쪽 넘김을 끝내고 그 앞의 쪽만 처리하고(`fetched 3, saved 2`), 제공자가 응답 없이 끊으면 run이 다시 시도되어(attempt 2) 성공한다. **변이 확인:** 카테고리 26의 요리 보정을 끄자 `["music","beauty",...]` 대 `["music","cook",...]`로 실패했다. 단위 시험: 쿼리 endpoint 설정·URL 인코딩·허용 이름·비밀 이름·지문(`crates/aip-execution/tests/integrations.rs` 2건), 서비스 설정 검사, 장부 단위 시험의 쿼리 endpoint 사례(`mysql_http_effects_...`: 응답을 잃은 GET은 의도를 지우고 `DELIVERY_FAILED`, 허용하지 않은 이름은 보내지 않음), job `create`의 정의 검사(`mongo_access.rs`)·요청 모양(`job_create_tests`)·안정 key 요구.

**알려진 한계.** (1) 새 영상 하나가 ctx 호출 3회(행, 문서 둘)라서 새 영상 200개를 한 번에 저장하려면 `limits.job_calls`가 700 근처여야 한다. (2) 원본은 Mongo 쓰기가 실패해도 행을 남기고 계속하지만 여기서는 job이 다시 시도되고 장부가 행을 재생한다. (3) 존재 확인(`youtube_url.in`)은 Video 행 정책이 보여 주는 영상만 본다. soft delete된 영상은 만들기 시도에서 `ALREADY_EXISTS`로 걸러지며 호출 1회를 쓴다. (4) 실제 YouTube credential이 없어서 live smoke는 미실행이다(가짜 제공자만). (5) 카테고리 보충(`YoutubeCategoryFillJob`)은 다음 조각이다.

## 2026-10-22 WBS 7.2.7 조각 A: 추천 풀 캐시의 예약 갱신(durable job의 Mongo·Redis 쓰기) (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.7은 진행 중)

**범위.** 원본 Celery beat 작업 `rebuild_recommendation_pool`(30분 주기, `HomeService._build_and_cache_ranked_pool`)을 durable job `rebuildRecommendationPool`로 옮겼다. 이를 위해 플랫폼에 세 가지를 더했고 결정은 [DECISIONS](../docs/DECISIONS.md)의 D-P-JOBSTORE다. (1) job이 `access mongo.<collection>`·`access redis "<키 또는 접두어:*>"`로 쓰기를 선언한다. 쓰기는 안정된 key와 함께 effect 장부에 의도를 먼저 커밋하고 결과를 같은 fenced transaction에 기록하며, 재시도는 기록된 결과를 재생한다. 응답을 잃은 쓰기는 다시 보내도 같은 데이터가 되는 명령(`set_ex`·`replace_hash_ex`·`del`·`expire`, `$set`·`$setOnInsert`만 쓰는 `update_one`, `delete_one`)만 같은 key로 다시 보내고 나머지는 `EFFECT_UNKNOWN`으로 두어 반복하지 않는다. job에는 saga 보상이 없다. (2) job의 ctx 호출 상한을 서버 설정 `limits.job_calls`(기본 64, 최대 2000)로 올릴 수 있다. (3) Python·Node worker의 쓰기 helper가 job 효과 key를 받는다(`effect_key=`, `{key}`). 같은 작업에서 `mysql_service`가 job을 서빙하지 않는데(worker 설정 없음) 계약 지문을 job 포함 변형으로 출력하던 기존 결함을 고쳤다(`/read`가 `CONTRACT_MISMATCH`). 읽기 확장 `Video.homeCategory`·`homePersonalized`는 캐시를 먼저 읽고, 비었거나 만료됐거나 Redis가 응답하지 않으면 요청 안에서 풀을 만든다(원본의 `_get_ranked_pool`과 같은 순서). `Video` 읽기 예산에 `cursor`를 더했다.

**원본과의 대조(`reference/slice14a_oracle.py`, 격리 MySQL 8.4 34391·MongoDB 8.0 15441·Redis 7.4 15442, 파이썬 3.13 venv에 Flask 3.0.2 등).** 같은 데이터에서 고정 원본(`7948b7e7`)의 `HomeService._build_and_cache_ranked_pool()`을 자체 Redis 데이터베이스 1에 쓰게 돌리고, 실제 `aip service serve`에 `rebuildRecommendationPool`을 관리자(`bob`)로 enqueue해 데이터베이스 2에 쓰게 한 뒤 비교했다: 풀 19개 항목(순서 포함), 카테고리 6개의 모든 항목, 카테고리 순서, 두 키의 만료(7000초 초과 7200초 이하)가 모두 같았다(`PASS` 5줄, `19 pool entries and 6 categories matched the original`). 첫 실행에서 카테고리 순서가 달랐는데, 원본의 캐시 hash는 값이 64바이트를 넘으면 해시 테이블이라 Redis 내부 순서로 돌아오기 때문이다. 원본이 캐시 없이 돌려주는 순서(원본이 반환한 카테고리 목록의 순서)가 기준이고, job이 그 순서를 `facereview:reco:category_order`에 따로 저장한다.

**실서버 종단 시험.** `crates/aip-cli/tests/mysql_facereview_pool_job.rs`(`1 passed`, 약 60초): ① 캐시가 빈 채로 golden 41건이 같고 읽기가 캐시를 쓰지 않는다 ② 회원(`alice`)은 job을 시작할 수 없고(`allow isAdmin(actor)`) 같은 key는 한 run으로 수렴한다 ③ job이 원본의 두 키와 순서 키를 쓰고 만료가 7200초 이하이며 장부에 3건이 SUCCEEDED다 ④ 캐시가 찬 뒤에도 golden 41건이 같고, 데이터를 바꿔도(영상 soft delete) job을 다시 돌릴 때까지 목록은 캐시 그대로이며, 다시 돌리면 빠지고, 캐시를 지우면(만료) 요청이 풀을 다시 만든다 ⑤ Redis를 끊으면(프록시) 읽기는 폴백으로 golden과 같고, 그 동안 시작한 job은 `RETRY_LATER`로 attempt 1이 끝나고 Redis가 돌아온 뒤 attempt 2에서 SUCCEEDED다 ⑥ 영상 3500개 추가(RDB 36쪽 + Mongo 36묶음): `limits.job_calls` 400이면 풀 1000개로 성공하고 캐시가 찬 채로는 요청이 답하며, 캐시가 비면 요청은 64 ctx 호출에 걸려 실패하고 job으로 다시 채운다 ⑦ 기본 상한 64로 같은 카탈로그는 `WIRE.COST_EXCEEDED`로 FAILED이고 이전 캐시는 그대로다 ⑧ 설정한 schedule(`* * * * *`, 서비스 principal)이 슬롯 key `schedule:pool:<slot>`으로 job을 돌려 캐시를 채우고 golden이 같다. **변이 확인:** 확장이 캐시를 읽지 않게 하자 ④의 "캐시된 목록은 job이 쓴 그대로" 단언에서 실패했다. 장부 단위 시험 `mysql_jobs::tests::mysql_external_writes_are_journaled_and_only_repeatable_unknowns_are_sent_again`(실제 MySQL 8.4, 기록용 Redis·Mongo 대역)은 의도 커밋·재생·`IDEMPOTENCY_MISMATCH`·선언 밖 접근·키 없음·saga 거부·읽기는 장부 없음·저장소 거절은 의도 삭제·반복 가능한 쓰기는 `ADAPTER_UNAVAILABLE`로 되돌려 같은 key 재전송·카운터와 `$inc`는 `UNKNOWN`으로 반복 금지·호출 상한(63회째까지 통과, 64번째 `COST_EXCEEDED`)·임차권 상실을 확인한다. **변이 확인:** 반복 가능 목록에 `incr_expire`를 넣자, 그리고 반복 가능 판정을 지우자 각각 실패했다.

**시험이 찾은 것.** (1) 읽기 예산의 비용이 `limit x (1 + 집계 수) + offset`이라 집계 둘을 고르는 `Video` 읽기는 offset 700 근처에서 `COST_EXCEEDED`가 되어, 읽기 확장의 "5000개 상한"이 실제로는 800개 근처였다. `Video` 읽기에 `cursor`를 선언하고 확장이 keyset으로 읽게 했다. (2) 읽기 확장은 64 ctx 호출이라 요청 안에서 풀을 만드는 폴백이 약 3000개에서 `WIRE.CALL_LIMIT`로 끝난다(원본은 무제한). 캐시가 찬 동안은 영향이 없다. (3) 위의 계약 지문 결함.

**전체 검증.** `tools/verify.sh`의 cargo 단계를 같은 순서로 돌린 스크립트(`fmt --check`, clippy 11종 `-D warnings`, `cargo test --workspace`, 엔진 6개): 앞의 19단계(fmt, clippy 11종, `cargo test --workspace`, aip-crypto, PoC 런타임, spike-v1·v2·v3 시험)는 한 번에 통과했다. 20단계(spike-v4-worker)는 내 환경에 `AIP_TEST_MONGO_URL`·`AIP_TEST_REDIS_URL` fixture가 없어 `native_capabilities` 5건이 환경 오류로 실패했고(코드 실패 아님), 인증 있는 MongoDB 7(15441)·Redis 7.4(15442)·별도 mongod(25443)를 띄워 spike-v4·v5·v6를 다시 돌려 모두 exit 0, prototype `cli` 13 passed다. 단계 뒤의 Node 시험과 Python unittest는 이 변경과 닿지 않아 미실행: 이유는 이번 Run이 cargo 단계만 재현했기 때문이다.

**알려진 한계.** (1) 세 쓰기(풀·카테고리·순서)는 한 트랜잭션이 아니다(원본의 Redis pipeline도 프로세스가 죽을 때는 원자적이지 않다). 풀만 있고 카테고리가 없으면 요청이 카테고리를 직접 만든다. (2) job은 최대 15000개 영상을 읽고 넘으면 크게 실패한다(`limits.job_calls`는 약 `영상 수 / 50`). (3) job의 Mongo·Redis 쓰기 principal은 정수 actor 핸들이라 `user_id`로 소유가 갈리는 컬렉션(`youtube_watching_data`)을 UUID wire 사용자 대신 쓸 수 없다. (4) PostgreSQL job 경로는 연결하지 않았다. (5) Node worker의 새 helper는 문법 검사만 했다. (6) YouTube 수집(7.2.7의 나머지)과 `limits.job_calls`의 운영 문서는 다음 조각이다.

## 2026-10-22 WBS 7.2.5 조각 B: Mongo·Redis 쓰기 capability, 외부 효과 saga와 보상 (구현, 로컬 실DB·실서버 통과)

**범위.** 쓰기 확장이 관계형 트랜잭션 옆에서 MongoDB·Redis에 쓰고, 그 트랜잭션이 커밋되지 않으면 되돌리는 경로를 만들었다. 원본의 `common/saga`(SagaContext·saga log)와 같은 의미다. (1) 문법: `access redis "<키 또는 접두어:*>"`를 읽기·쓰기 확장에 선언하고(`parser.rs`·`sema.rs`, `DEF.BAD_CAPABILITY`), 쓰기 확장은 `mongo.<collection>`도 선언한다. (2) 쓰기 명령: Mongo `insert_one`·`update_one`·`find_one_and_update`·`delete_one`(컬렉션별 필드 허용 목록, `$set`·`$setOnInsert`·`$inc`만, 값 안 `$` 키 금지, 1MB, `youtube_watching_data`는 서버가 `user_id`를 인증 actor로 강제), Redis `set_ex`·`set_nx_ex`·`delete`·`getdel`·`incr_expire`·`expire`·`push_trim_expire`·`replace_hash_ex`(TTL 1초~30일, 값 1MB)와 읽기 `lrange`. (3) `compensate=True`를 준 호출은 되돌릴 기록을 남긴다(삽입은 삭제, upsert는 새 행 삭제 또는 이전 문서 복원, 삭제는 다시 삽입, Redis는 이전 값과 남은 수명 복원). (4) `ExternalEffects`(`spikes/spike-v4-worker/src/external.rs`)가 효과 순서·보상·saga log를 맡고, 서버(`spike-v6-transport/src/lib.rs`)가 확장 실패·관계형 검사 실패·커밋 거절이면 최신 단계부터 보상, 커밋 결과 불명(`WIRE.COMMIT_UNKNOWN`)이면 보상하지 않고 log를 `unknown`으로 남기며, 보상이 하나라도 실패하면 새 코드 `WIRE.COMPENSATION_FAILED`로 단계 이름을 알린다. saga log는 원본 `saga_transaction_log` 문서 모양이고 첫 보상 가능 효과 전에 쓰며, Mongo가 log를 못 받으면 그 효과는 일어나지 않는다. (5) 설정: `workers.mongo.write_collections`(허용 목록, 비면 읽기 전용)와 `ensure_indexes`. (6) Python·Node worker의 `ctx.mongo`·`ctx.redis`에 쓰기 메서드를 더했다.

**실제 서비스 시험(MongoDB 7, Redis 7.4, MySQL 8.4 컨테이너).** `cargo test --test native_capabilities`(spike-v4-worker) 6 passed. 새 시험 둘: Mongo 쓰기(원본 첫 프레임 upsert를 센티초 경로 `emotion_score_timeline.2029`와 `$inc` 합계로 재현하고 같은 프레임 재생 시 `frame_count`가 2가 됨을 확인해 중복 방지는 호출자(Redis SET NX) 몫임을 못박음, `$date`가 BSON 날짜로 저장됨, 다른 회원의 필터·스푸핑 `user_id`·`$ne`·알 수 없는 연산자·모델 밖 필드·깊은 경로·너무 큰 문서·빈 filter 거절, `saga_transaction_log`에는 가짜 grant로도 쓸 수 없음, 고유 인덱스 충돌은 `WIRE.CONFLICT`, 삽입·upsert 신규·upsert 기존·삭제의 보상이 문서를 Int32·날짜 형식까지 포함해 이전과 같게 되돌리고 보상을 두 번 해도 무해, 저널은 transaction당 한 문서로 갱신)와 Redis 쓰기(TTL 1초 만료 확인, `set_nx_ex`로 첫 호출만 성공하고 만료 뒤 다시 성공, 보상 뒤 같은 프레임을 다시 기록 가능, `delete`·`getdel`·`set_ex`의 보상이 값과 남은 수명(90~100초 범위)을 복원, 지표 카운터의 창과 `push_trim_expire`가 최신 3개를 유지, 900KB 풀 캐시 값과 해시 전체 교체, `lrange` 창, TTL·크기·네임스페이스·정확 키 grant 경계). 문법은 `spikes/spike-v1-fixture/tests/mongo_access.rs` 6 passed(새 3). 호스트 경계는 `worker_action_external_writes_reach_the_host_only_through_a_declared_grant`(선언된 grant만 호스트에 닿음, 실패하면 확장이 성공으로 돌려줘도 거절, 20회 상한에 외부 쓰기 포함)가, 보상 순서·log는 `external::tests` 7 passed(최신 단계부터 보상, 보상 하나가 실패해도 나머지를 시도하고 log에 이름이 남음, log가 안 되면 효과가 시작하지 않음, 효과 뒤 log가 실패하면 즉시 되돌림, 커밋 불명은 보상하지 않음)가 맡는다.

**실서버 종단 시험.** `crates/aip-cli/tests/mysql_external_effects.rs`가 실제 `aip service serve`·Python 샌드박스·MySQL·MongoDB·Redis로 8가지를 확인했다(`1 passed`, 3.4~4.4초): ① 성공(관계형 변경, Mongo upsert, 중복 방지 키, `completed` log 3단계) ② 같은 key 재생과 같은 프레임 새 key가 아무것도 두 번 세지 않음 ③ 확장이 효과 뒤 실패하면 관계형 변경과 Mongo·Redis 효과가 모두 되돌려지고 log가 `compensated`(되돌릴 수 없는 카운터는 남음) ④ 관계형 검사 실패(조회수 상한)도 같음 ⑤ 다른 회원의 세션과 고유 id가 충돌해 Mongo가 실패하면 앞선 Redis 효과가 취소됨 ⑥ Mongo 불통이면 log가 시작하지 못해 효과가 없고, 복구 뒤 같은 요청이 성공 ⑥b Redis 불통도 같은 방식으로 실패하고 복구 뒤 성공 ⑦ COMMIT 응답이 사라지면(프록시) `WIRE.COMMIT_UNKNOWN`, MySQL은 커밋됐고 효과는 남으며 log가 `unknown`, 같은 key를 다시 보내면 저장된 결과를 재생. **변이 확인:** 확장 실패 경로의 롤백 호출을 빼자 ③에서 `the Mongo upsert was undone`(left Some(1), right None)으로 실패했고 되돌리자 통과했다.

**전체 검증 스크립트.** `cargo fmt --all --check`, 워크스페이스·엔진 clippy 11종 `-D warnings`, `cargo test --workspace --locked`와 엔진 6개 테스트를 `tools/verify.sh`와 같은 순서로 돌렸다. 첫 실행에서 `spike-v4-worker`의 `principal_uuid_mongo`만 `AIP_TEST_MONGO_UUID_URL`(CI가 쓰는 별도 mongod) 없이 돌려 실패했고, 그 컨테이너를 띄워 처음부터 다시 돌리자 18단계(fmt, clippy 11종, 워크스페이스 테스트, 엔진 6개 테스트)가 모두 `exit=0`이었다. `tools/verify.sh`의 node·python 제품 시험과 `aip-crypto`·`poc-runtime`·`prototype` 단계는 이 변경이 닿지 않는 코드라 미실행이다.

**Linux CI.** cee2d8a(조각 A)의 Verify 39644152426이 completed/success였고, c621143(조각 B)의 Verify 39650801406이 `scripts/verify-mysql-worker-e2e.sh run-ci`에서 `Running the watch detail end-to-end test`(조각 A)와 `Running the external effects end-to-end test`를 실행해 completed/success다(스크립트가 `1 passed`를 grep으로 강제한다). 같은 SHA의 Database contracts·Dependencies·Object storage·External brokers·App packages·Linux worker jobs·Linux cgroup v2·Deployment checks도 completed/success다.

**알려진 차이·한계.** (1) 원본 saga는 첫 보상 실패에서 멈추고 `failed`로 표시하지만 여기서는 나머지 단계도 시도하고 실패한 단계 이름을 돌려준다. 원본의 `union_transactional`은 SQL 롤백 뒤 보상하고, 여기서는 관계형 트랜잭션이 커밋되지 않았음이 확실할 때만 보상한다. (2) 커밋 결과가 불명이면 보상하지 않는다(원본은 이 경우를 구분하지 않는다). log에 `unknown` 상태를 더했다. (3) log 쓰기는 Mongo가 있어야 한다. Redis만 쓰는 호출은 log 없이 메모리에서 보상한다. 프로세스가 커밋 전에 죽으면 `in_progress` log가 남고 보상은 자동으로 이어지지 않는다(복구 작업은 이 항목 밖). (4) `compensate`를 주지 않은 호출(프레임 upsert, 카운터, 목록)은 되돌리지 않는다. 원본의 `upsert_frame`도 그렇다. (5) 쓰기 확장은 Mongo·Redis를 쓰기만 한다(읽기는 `find_one_and_update`의 반환 문서). (6) 원본의 Redis 키 `facereview:session:*`는 설정 검증이 `session` 접두어를 막는다(서버 소유 세션-principal 매핑이 없다는 기존 결정). 이름만 다르게 정하면 같은 역할을 한다. (7) Node worker의 새 메서드는 문법 검사만 했다(미실행: Node worker 종단 시험). (8) 일반 `bson` Int64는 쓰기 변환에서 Int32로 들어갈 수 있다(원본 Python 드라이버와 같은 규칙). (9) 타임라인 키 순서는 JSON 객체가 정렬되어 쓰는 순서가 원본과 다를 수 있다.

## 2026-10-22 WBS 7.2.5 조각 A: `GET watch/video` 전 시청자 타임라인 집계와 조회수 전이 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.5는 진행 중)

7.2.3이 넘긴 두 가지를 풀었다. (1) Mongo capability에 **파생 컬렉션** `video_emotion_timeline`을 더했다. 확장이 `access mongo.video_emotion_timeline`을 선언하고 `{command: "timeline", video_id, duration}`을 부르면 서버가 그 영상의 모든 시청 세션을 읽어 원본 `_get_compressed_timeline_data`와 같은 계산(센티초 키별 세션 평균, `duration`초를 100버킷으로 나눈 키 평균, 소수 한 자리 반올림)을 하고 100개 버킷 평균만 돌려준다. 세션 문서·user_id·세션별 값은 나가지 않고, 이 grant는 원본 `youtube_watching_data`(호출자 본인 전용)를 열지 않는다. (2) `Video.view` 전이(`view_count = this.view_count + 1, updated_at = now`, 삭제된 영상 제외)와 `expose apply view`를 더해 원본이 GET 안에서 하던 증가를 호출자가 읽기 뒤에 명시 호출한다. 정의에 `Video.watchDetail` 읽기 확장이 생겨 원본 `VideoDetailDto`(좋아요·북마크 여부, 좋아요·댓글 수, 타임라인 포함)를 한 번에 돌려준다.

정의 언어가 증감 대입을 null이 아닌 Int에만 허용해서, `video.view_count`를 NOT NULL로 바꾸는 DBA 단계(`db-defaults.sql`: NULL 보정 후 `NOT NULL DEFAULT 0`)를 더했다. 원본은 NULL을 쓰지 않고 NULL이면 500이다. 이 변경이 기존 대조에 영향을 주지 않는지 `mysql_facereview_reads`·`recommend`·`admin`·`auth`를 같은 컨테이너에서 다시 돌려 모두 `1 passed`였다.

**대조(실제 `aip service serve`, 격리 MySQL 8.4 35441·MongoDB 7 35442).** `reference/slice13f_oracle.py`가 같은 데이터에서 고정 원본(`7948b7e7`)의 `WatchService.get_video_detail`을 직접 실행해 비교했다. 영상 12개(60초 영상 다세션, 길이 NULL, 길이 0, 세션 없음, 3초 영상, 시드 영상, 삭제·NULL 플래그·없는 영상)와 호출자 3명(36건)에 `view` 쓰기 5건과 증가 뒤 재읽기 1건을 더해 `42/42 cases matched`다. 원본 `get_video_detail`이 `@transactional`이라 자기 증가분을 커밋하므로 오라클이 증가분이 정확히 +1임을 확인한 뒤 행을 되돌려 양쪽이 같은 데이터를 읽게 했다. 같은 명령 3회 `slice13f-golden.json`이 동일(`cmp`)했다. 세션 시드는 고정 RNG(`slice13f_make_seed.py`)이고 세션 19개, 타임라인 항목 830개(무작위 소수 둘째 자리 점수)에 짧은 배열·숫자 아닌 키·빈 타임라인·정수 점수를 포함한다.

**시험이 찾은 것.** 첫 실행은 39/42였다. 3초 영상의 한 버킷이 원본 48.1, 이 포트 48.2였다. MongoDB의 `$sum`은 합산 오차를 보정해서 원본의 단순 float 합과 마지막 비트가 달랐고, 그 값이 반올림 경계에 있었다. 서버 쪽 합산을 원본과 같은 순서의 단순 합으로 바꾸자(세션은 natural order, 세션 안 키는 저장 순서) 42/42가 되었다. 정의 언어는 nullable Int 증감을 거절하고 어댑터는 `NOT NULL`이 아닌 열을 NOT NULL 필드에 연결하지 않아서(`nullable 불일치`) 두 번 막힌 끝에 위의 DBA 단계를 택했다.

**변이 확인.** 반올림을 내림(`floor(x*10)/10`)으로 바꾸자 `mysql_facereview_watch`가 `watch/video alice timeline 60s: timeline`에서 60.0 대 60.1로 실패했고, 되돌리자 통과했다.

**명령(로컬).** `cargo test --test native_capabilities`(spike-v4-worker, 실제 Mongo 7·Redis 7.4 컨테이너) 4 passed(새 `timeline_collection_aggregates_all_viewers_without_exposing_sessions`: 두 시청자 합산, 깨진 세션·짧은 배열 건너뜀, 세션 필드가 응답에 없음, grant 없이는 `WIRE.ACCESS_NOT_DECLARED`, 원문 명령·추가 입력·음수 duration은 `WIRE.BAD_VALUE`, 이 grant로 원본 컬렉션 거절). `cargo test --lib -- timeline one_decimal` 2 passed. `cargo test -p aip-cli --features mysql --test mysql_facereview_watch -- --ignored`가 `1 passed`(골든 재생 36건과 `view` 두 번 뒤 조회수 12, 삭제·없는 영상 `WIRE.MISSING_TARGET`, 손님 `WIRE.UNAUTHENTICATED`). `cargo fmt --all --check`와 변경한 크레이트의 `clippy -D warnings`(aip-cli mysql·poc-runtime, spike-v4-worker, spike-v1-fixture, workspace lib·bins)는 경고 없이 통과했다.

**알려진 차이·한계.** 위 README의 "Slice13-F"와 같다: 타임라인이 없는 영상의 기본 타임라인, 세션 50000개·항목 5000000개 상한(원본은 무제한), 음수·비유한 키 건너뜀, 비객체 타임라인(원본은 500)과 숫자가 아닌 점수(실패)의 처리, 없는/삭제된 영상은 `WIRE.EXTENSION_ERROR`, 손님은 확장 경로가 거절한다. Mongo 쓰기·Redis 쓰기·saga는 이 조각에 없다(7.2.5의 다음 조각).

## 2026-10-21 WBS 7.2.9 관리자 목록 읽기: 회원·영상 요청·영상·댓글 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.9는 미완)

원본 `AdminService.get_users`·`get_video_requests`·`get_videos`·`get_comments`가 하는 읽기를 정식 정의로 옮겼다. 정의 변경: `User`에 행별 집계 `watchCount`(시청 기록 행 수)·`commentCount`(삭제된 댓글도 센다)와 관리자 전용 `totalCount`, `name.like`·`email.like` 필터, `VideoRequest`에 `traverse user`와 관리자 전용 `totalCount`, `Comment`에 `content.like`·`traverse video`와 관리자 전용 `totalCount`(`access adminOnly`). 총계는 호출자 필터를 받는 집계라 비관리자는 `WIRE.ACCESS_DENIED`다. 영상 목록은 7.2.3의 `likeCount`·`commentCount`와 `Video.totalCount`를 그대로 쓴다.

**대조(실제 `aip service serve`, 격리 MySQL 8.4 34411).** `reference/slice13e_oracle.py`가 원본 11개 표에 `seed.sql`·`slice13d-seed.sql`·`slice13e-seed.sql`(회원 25명 중 3명 탈퇴, 댓글 40개 중 일부 삭제와 삭제된 영상 포함, 요청 30개 세 상태, 시청 기록 60개)을 넣고, 같은 데이터에서 고정 원본의 `AdminService`를 직접 실행해 `/read`와 `/read` 총계로 만든 같은 JSON 모양과 비교했다. 관리자 두 명(ADMIN, SUPER_ADMIN)마다 회원 9개·요청 5개·영상 7개·댓글 8개 상황(페이지 넘김, 이름·이메일 키워드, 탈퇴 필터 조합, 일치 없음, 마지막 페이지 너머, 상태·카테고리·영상·삭제 필터)과 일반 회원의 총계·목록 거절 4건, 합계 `62/62 cases matched the original`이다. 한 가지는 의도된 차이다(아래). 처음 실행은 36/62였다. 요청·댓글의 연관 행 요청 문법(`traverse`가 아니라 `select` 안의 객체)을 잘못 쓴 것이 원인이었고 고치자 60/62가 되었으며 남은 둘이 아래 차이다. CI 시험 `crates/aip-cli/tests/mysql_facereview_admin.rs`가 같은 서버에서 golden의 58개 요청(`slice13e-golden.json`)을 새 배포에 재생한다. 로컬에서 `1 passed`(33초)이고, 회원의 댓글 수 집계가 삭제된 댓글을 빼도록 바꾸면 `users bob all p1`에서 실패함을 확인했다. 같은 정의로 `mysql_facereview_reads`·`auth`·`recommend`도 각각 `1 passed`, `spike-v1-fixture` `advisories`·`mongo_access`와 `spike-v2-read` `wbs723_wire_or_verification`도 통과했다.

**Linux CI(6604a82).** Verify 39613541698이 `scripts/verify-mysql-worker-e2e.sh run-ci`에서 `mysql_facereview_admin`을 실행해 `1 passed`(11.21초)다(로그: `Running the administrator list end-to-end test`). 같은 SHA의 Database contracts·Dependencies·Object storage·External brokers·App packages·Linux worker jobs·Linux cgroup v2도 completed/success다.

**알려진 차이·미구현.** (1) 삭제됐거나 `is_deleted`가 NULL인 영상은 `Video` 행 정책이 관리자에게도 숨긴다(정책을 풀면 `Video` 읽기·추천 풀·`totalCount`가 모두 달라진다). 그래서 그런 영상 댓글의 `video_title`은 null이고, `video` 필터로 그 영상의 댓글을 찾으면 Ref 필터 규칙 때문에 비어 나온다(필터 없는 목록에는 `video_id`와 함께 있다). 원본은 둘 다 보여 준다. (2) 원본의 JSON 모양(`users`·`total`·`has_next` 등)은 서버 응답이 아니라 호출자가 만든다. (3) 원본의 `GET admin/users`가 키워드를 `LIKE %kw%`로 쓰는 것과 같게 `like`를 썼고 `contains`와 와일드카드 처리가 다르다. (4) **아직 없음**: 영상 요청 승인(영상 생성과 Mongo 초기 분포 쓰기), `admin/dashboard/*`, `admin/system/status`, `admin/dummy-data`와 `POST home/video/recommend`. 이 절은 7.2.9의 완료가 아니다.

## 2026-10-21 WBS 7.2.3 추천 목록 포팅: `home/category`·`home/personalized`·`watch/recommended` (구현, 로컬 실DB·실서버·원본 서비스 대조와 Linux CI 통과)

**범위.** 원본 `HomeService.get_videos_by_category_emotions`·`get_personalized_videos`와 `WatchService.get_recommended_videos`를 읽기 확장(`Video.homeCategory`·`homePersonalized`·`watchRecommended`)으로 옮겼다. 계산은 이미 원본과 대조된 `recommendation_math.py`·`recommendation_pool_builder.py`이고, 확장 `slice13d_recommend.py`가 원본이 읽는 입력(활성 영상, Mongo 분포, 좋아요 수, 본인 선호 장르·시청 기록·최근 세션)을 모아 같은 모양으로 답한다. 서버가 직접 서빙하도록 제품 쪽도 세 가지를 고쳤다. (1) 정의 문법이 `access mongo.<collection>`을 선언하지 못해 확장이 `aip service serve`로는 Mongo 권한을 가질 수 없었다(Slice13-B는 프로세스 안 호스트에서 facts를 덧붙여 우회). 이제 읽기 확장이 `mongo.<컬렉션>`을 빈 grant로 선언하고, 중복·`mongo`라는 resource 이름 충돌은 정의 오류다(`spikes/spike-v1-fixture/tests/mongo_access.rs` 3개, 변이로 기능을 빼면 2개 실패). (2) 원본의 좋아요·댓글 수는 모든 호출자에게 같은 공개 값인데 `VideoLike`는 본인 행만, `Comment`는 관리자에게 삭제 행까지 보여서 정책 안의 읽기로는 만들 수 없다. `Video`에 행별 집계 `likeCount`·`commentCount`(`where is_deleted = false`)를 더했고 비회원·회원·관리자가 같은 값을 받는 것을 `mysql_facereview_reads`로 확인했다(`where`를 지우면 관리자 영상의 댓글 수가 2가 아니라 3으로 실패). (3) 호스트 Mongo 프로젝션 허용 목록에 `emotion_percentages`를 더했다. 음수 프로젝션은 허용 목록의 키만 쓸 수 있어 수 MB의 `emotion_score_timeline`을 뺄 수 없고, 문서 전체는 1MB 결과 상한을 넘기기 때문이다. `client_info`는 계속 읽을 수 없다(`native_capabilities`, 필드를 빼면 실패).

**원본과의 대조(2026-10-21, 실제 `aip service serve`).** 명령: `reference/slice13d_oracle.py`(환경 `S13D_*`는 파일 머리말). 격리 MySQL 8.4(34411)와 MongoDB 8.0(34413)에 원본 11개 표 스키마, `seed.sql`, `slice13d-seed.sql`, Mongo 문서 2종을 넣고, 같은 데이터에서 고정 원본(`7948b7e`)의 `HomeService`·`WatchService`를 Flask 앱 컨텍스트로 직접 실행해 `workers.mongo`·Python worker 샌드박스·JWKS 검증 bearer 토큰으로 `POST /extension`에 응답하는 서버와 비교했다. 결과: `41/41 cases matched the original`(카테고리 목록 4명, 개인화 5명 중 전부 본 회원은 빈 목록, 연관 영상 14개 상황 x 2명과 정의 없는 정식 영상, 모르는·삭제·NULL 플래그 영상은 양쪽 모두 거절). 같은 명령을 세 번 돌려 쓴 `slice13d-golden.json`이 바이트까지 같았다. 시드는 시간·우연에 흔들리지 않게 짰다(점수된 영상은 모두 2025년 생성이라 신선도 항이 바닥값, 카테고리마다 패딩 후보가 최대 1개, 풀이 150 미만). 처음 35개 사례 중 11개만 맞았다. 연관 영상 전부가 확장의 `!=` 필터(문법에 없어 `WIRE.FILTER_NOT_ALLOWED`)로 실패했고 읽기 뒤에 현재 영상을 빼도록 고치자 34/35가 되었다. 남은 개인화 1건의 원인은 원본의 `user.favorite_genres.all()`이 정렬 없는 질의라서 MySQL이 유일 인덱스 `(user_id, genre)`로 답해 선호 순위가 고른 순서가 아니라 ENUM 선언 순서가 된다는 것이었다. 포팅이 이를 재현하게 고쳐 35/35가 되었고(고르지 않게 되돌리면 CI 시험이 `home/personalized alice`에서 실패함을 확인), 연관 영상 시드를 넓혀(같은 감정·동점·페이지 넘김) 41/41로 늘렸다. Slice13-B 대조(`slice13b_oracle.py`, 정식 정의를 그대로 쓰도록 단순화)는 `36 oracle cases, 0 failure(s)`로 회귀가 없었다.

**CI 시험.** `crates/aip-cli/tests/mysql_facereview_recommend.rs`가 같은 서버·Mongo·worker 경로에서 golden의 41개 요청을 새 배포에 재생한다(원본 체크아웃·Flask 불필요). 로컬(`AIP_TEST_MYSQL_URL`=34411, `AIP_TEST_MONGO_URL`=34413)에서 `cargo test -p aip-cli --features aip-cli/mysql --test mysql_facereview_recommend -- --ignored`가 `1 passed`(약 35초)다. 비회원은 `WIRE.UNAUTHENTICATED`, 선언 밖 입력 키와 `page` 0은 거절이다. `scripts/verify-mysql-worker-e2e.sh run-ci`가 이 시험을 인증 시험 뒤에 실행한다(Verify의 native-stores MongoDB 사용). 정식 정의가 확장을 선언하므로 워커 디렉터리에 Python 모듈이 없으면 `serve`가 시작을 거부한다. 이를 반영해 `mysql_facereview_auth`가 모듈 4개를 복사하게 했고 `mysql_facereview_reads`·`mysql_facereview_auth`가 각각 `1 passed`다. 원본 체크아웃이 CRLF로 전달돼 모든 파일이 수정으로 보이므로 오라클의 청결성 검사는 줄바꿈을 무시한 내용 차이로 바꿨다.

**Linux CI(c85c60b).** Verify 39607862531이 Bubblewrap 안의 Python worker와 native-stores MongoDB로 `mysql_facereview_recommend`를 실행해 `1 passed`(16.28초)다(로그: `Running the recommendation end-to-end test`). 같은 SHA의 Database contracts·Dependencies·Object storage·External brokers·App packages·Linux worker jobs·Linux cgroup v2도 모두 completed/success다. 앞선 두 문법·집계 커밋(`1c6df5e`, `acfe3b0`)과 Mongo 프로젝션 커밋(`71e6b2b`)도 각 SHA에서 success였다.

**알려진 한계·미구현.** (0) Redis 풀 캐시를 쓰지 않고 요청마다 풀을 만든다(원본이 캐시가 비었을 때 하는 경로). 읽기 확장은 캐시를 쓸 수 없고 오프라인 갱신은 7.2.7이다. (1) 활성 영상 5000개, 본인 시청 기록 5000행을 넘으면 조용히 자르지 않고 실패한다. (2) 풀 구성 중 패딩, 개인화의 탐색 표본, 대체 표본은 원본의 `random`이라 재현되지 않는다(시험은 한 가지 결과만 나오는 시드로 대조). (3) 모르는·삭제·NULL 플래그 영상은 원본의 404가 아니라 `WIRE.EXTENSION_ERROR`다. 비활성 호출자는 USER_NOT_FOUND가 아니라 정책에 따른 빈 목록이다. (4) `/extension`은 서명된 호출자를 요구하므로 원본의 `login_optional` 비회원은 받을 수 없다(`home/category`, `watch/recommended`). (5) **옮기지 않은 것**: `GET watch/video`는 모든 시청자의 `emotion_score_timeline` 평균인데 Mongo capability는 `youtube_watching_data`에서 본인 세션만 주도록 막혀 있고, 쓰기 확장은 전이만 부를 수 있어 원본처럼 GET 안에서 `view_count += 1`을 함께 할 수 없다(집계 capability 또는 파생 컬렉션과 명시적 조회수 전이가 필요, 7.2.5). `POST home/video/recommend`는 `VideoRequest.create`가 있으나 원본이 서버에서 `youtube_full_url`을 만들고, 누가 요청했든 같은 URL이면 거절하며(호출자는 남의 요청 행을 읽지 못한다), 한 트랜잭션에 묶는다. 계산된 기본값과 `video_request.youtube_url`의 유일 인덱스가 필요하다(7.2.9). 두 항목은 7.2.3의 완료로 세지 않는다. 이 절은 화면·SDK 전환(7.2.10)과 전체 journey 동등성(7.2.11)을 뜻하지 않는다.

## 2026-10-21 WBS 7.2.2 인증 포팅: 비밀번호 확장, 단회용 refresh 세션, `/auth/*` 로그인 경로 (구현, 로컬 실DB·HTTP와 Linux CI 통과)

**대조한 현재 경로(코드 확인, 서버 실행 아님).** AIP 서버는 외부 IdP가 발급한 RS256 `at+jwt`(최대 900초)만 검증하고 주체를 `service principal bind`로 묶는다([운영](OPERATIONS.md): "로그인·refresh token은 제공자와 앱이 담당"). 서버에는 비밀번호 로그인·토큰 발급·refresh 경로가 없다. 확장(worker)은 검증된 principal로만 실행되므로 로그인 같은 사전 인증 호출을 받을 등록 경로가 없고, `operation`의 입력은 클라이언트가 정한다. 따라서 서버가 직접 읽은 해시를 확장에 넘겨 판정만 받는 서버 주도 호출이 필요하다. 고정 원본(new-facereview `7948b7e`)의 해시는 `werkzeug.security`(requirements `Werkzeug==3.0.2`, bcrypt는 선언만 되고 import되지 않음)이며, 원본 `reissue`는 받은 refresh token을 무효화하지 않고 `logout`은 Redis 블랙리스트에 넣을 뿐 `reissue`가 이를 보지 않아 재발급 replay가 허용된다. 이 결함은 복제하지 않는다.

1단계 `product/providers/password_auth.py`: Werkzeug 3.0.2 형식(`method$salt$hex`, scrypt와 pbkdf2)을 표준 라이브러리만으로 검증·생성한다. 실제 Werkzeug 3.0.2가 만든 해시 8종(scrypt 기본·소형, pbkdf2 sha1/256/512, 한글·이모지, 72자, `$` 포함)을 `fixtures/password-auth/werkzeug-3.0.2.json`에 고정했고 `python3 -m unittest product.tests.password_auth_test`는 7개 중 1개 skip으로 OK다. Werkzeug 3.0.2 venv에서는 `7 tests OK`이며 우리가 만든 해시를 원본 라이브러리가 검증하는 교차 시험이 실행된다. 저장된 해시는 데이터이므로 scrypt n 1024~2^17·r≤16·p≤4, pbkdf2 반복 ≤5,000,000, hash는 sha1/256/512로 제한하고 범위 밖·형식 오류·비문자 입력은 예외 없이 거부한다(원본은 임의 파라미터를 그대로 실행). 시드 fixture의 `pbkdf2:hash-a` 같은 가짜 값도 거부한다. 확장 진입점 `verify_password`는 판정과 재해시 필요 여부만 반환한다. 서버가 이 확장을 호출해 principal을 만드는 부분은 다음 단계이며 이번에는 연결하지 않았다.

2단계 `aip_migrate::mysql_auth_sessions`: `aip_auth_sessions`·`aip_auth_refresh_tokens` 두 표(부트스트랩과 재채택 DDL에 포함)와 `MysqlAuthSessions`. refresh token은 한 번만 쓰고 SHA-256만 저장하며 판정은 DB 시계로 한다. 쓴 토큰의 재사용은 세션 전체 폐기(원본과 다른 의도된 차이), 로그아웃·계정 정지(`revoke_subject`)·idle 7일·절대 30일 만료를 지원한다. 실 MySQL 8.4(34741)와 MariaDB 11.4(34742) 격리 컨테이너에서 `cargo test -p aip-migrate --features mysql --test mysql_auth_sessions -- --ignored --test-threads=1`이 양쪽 모두 `4 passed`다. 8개 동시 refresh 경쟁에서 정확히 1개만 통과한다. 변이 확인: 사용 표시를 지우면 2개, 행 잠금을 지우면 경쟁 시험이 실패했다. `cargo clippy -p aip-migrate --features mysql --all-targets -- -D warnings` 통과, `mysql_jobs`·`mysql_capability_gate` 회귀 `4 passed`·`6 passed`. 한 쪽 동시성 한계: 응답을 잃은 정상 클라이언트가 이미 쓴 토큰을 다시 보내면 replay로 보고 세션을 폐기한다(재로그인 필요). 유예 창은 두지 않았다.

3단계 서버 경로(`/auth/check-email·signup·login·reissue·logout`). 서버가 직접 계정을 확인하고 토큰을 만든다. 로그인 때 서버가 비밀번호 열(`secret_column`, 어떤 resource에도 매핑되지 않은 열이 아니면 시작을 거부)을 직접 읽어 `product/providers/password_auth.py`의 `verify_password`를 샌드박스 worker에서 실행하고 판정만 받는다(`operation read verifyPassword·hashPassword`, 입력은 서버가 구성, 클라이언트는 `allow false`로 호출 불가). 계정이 처음 로그인하면 서버가 계정 행의 UUID를 subject로 actor에 연결(principal bind)하며, 운영자가 `principal revoke`한 계정은 올바른 비밀번호로도 로그인할 수 없다(403 `account_disabled`). 접근 토큰은 서버의 RSA 키로 서명한 `at+jwt`(기본 300초, 최대 900초)이며 claim은 `iss·aud·sub·sid·iat·nbf·exp`뿐이다. 검증기는 기존 JWKS 경로를 그대로 쓰고, `sid`가 가리키는 세션이 살아 있고 subject가 일치할 때만 받는다. 서명 키의 공개 키가 `auth.jwks`에 없으면 서버 시작이 `CREDENTIALS_CONFIG`로 실패한다. refresh token은 `HttpOnly; Secure; SameSite=None; Path=/auth; Max-Age=604800` 쿠키로만 내려간다. 응답 본문에는 없다. 요청 본문은 알려진 키만 받아(`role`·`user_id`·`is_deleted` 등은 422 `unknown_field`) role은 설정의 `default_role`로 고정된다. 이 전송 계층은 `Content-Type: application/json`과 허용 origin 목록을 요구하고(자격 증명 CORS는 일치하는 origin에만), 본문 16KiB, POST만, `cache-control: no-store`다.

실행(2026-10-21, 격리 MySQL 8.4 34741, 실제 `aip service` 바이너리·macOS Seatbelt 안의 Python worker): `cargo test -p aip-cli --features mysql --locked --test mysql_facereview_auth -- --ignored --nocapture --test-threads=1`은 `1 passed`다. 원본 스키마(new-facereview@7948b7e 11개 표)에서 확인한 것: 실제 Werkzeug 3.0.2 scrypt 해시로 원본이 만든 계정이 로그인한다. 가입 거부 9종(허용되지 않은 `role`·`user_id`·`is_deleted`, 짧은 비밀번호, 이름 길이 2종, 장르 없음·모르는 장르, 이메일 형식)은 행을 만들지 않는다. 성공한 가입은 role `GENERAL`·`is_deleted=0`·scrypt 해시·장르 2행(중복 요청은 1행)을 만들고, 같은 이메일(대소문자만 다름)은 409다. 틀린 비밀번호·없는 이메일·가짜 해시의 응답 본문이 requestId를 빼면 같다. 원본 refresh replay는 복제하지 않았다. 쓴 refresh token을 다시 보내면 401 `refresh_token_reused`이고 같은 세션의 최신 토큰과 접근 토큰도 죽는다. 같은 토큰으로 8개 동시 reissue는 정확히 1개만 200이다. 로그아웃은 쿠키와 접근 토큰을 함께 죽이고 반복해도 같은 응답이다. 정지된 계정(`is_deleted=1`)은 다음 refresh에서 그 계정의 모든 세션이 폐기되고 로그인도 안 된다. `principal revoke`된 계정은 로그인 403, 기존 접근 토큰 401이다. 게스트는 `/session`이 익명 principal로 성공하고 `User` 읽기는 0행이다. 위조 4종(다른 사용자의 `sid`, `sid` 없음, 없는 세션, 모르는 서명 키)은 401이며, 서버 키로 서명하고 `role: SUPER_ADMIN`·`actorId`를 얹은 유효 세션 토큰도 GENERAL 계정의 읽기 범위를 넓히지 못한다. GENERAL 계정이 자기 role을 올리거나(`User.changeRole`) 관리자를 비활성화하는 `/apply`는 거부되고 표는 바뀌지 않는다. 전송: 허용되지 않은 origin은 `WIRE.ORIGIN_NOT_ALLOWED`이고 쿠키·자격 증명 헤더가 없으며, form 콘텐츠 타입 400, GET 405, 20KB 본문 413이다. 변이 확인: 폐기된 principal을 로그인으로 되살리는 변경과 검증기의 세션 확인 제거가 각각 이 시험에서 실패한다.

회귀: `cargo test -p aip-cli --features mysql --test mysql_adoption_flow --test mysql_facereview_reads -- --ignored --test-threads=1`은 `2 passed`·`1 passed`(principal bind를 공용 함수로 뽑은 변경 뒤), `--test codes --test error_codes`는 `11 passed`·`4 passed`, `spike-v6-transport --test wire_codes`는 `9 passed`, `aip-auth --lib` `19 passed`(로컬 발급기 5개 포함), `cargo clippy ... --features mysql,aip-cli/mysql --all-targets -- -D warnings`와 spike-v6 clippy는 경고 없음이다. 로컬 발급기 시험은 발급한 토큰이 외부 IdP 토큰과 같은 검증기를 통과하고 모르는 키·kid는 거부되며 PKCS#1·PKCS#8 PEM은 받고 암호화·공개 키 PEM은 거부함을 확인한다.

**동시 가입 결함(시험이 찾은 것).** 서로 다른 이메일 6개(같은 이메일 2개 포함)를 동시에 가입시키는 시험을 추가하자 약 3회 중 1회 한 요청이 503 `principal_bind_failed`였다. 원인은 코드로 확인했다. `principal bind`가 공유하는 함수가 아직 없는 `(issuer, subject)` 키를 `FOR UPDATE`로 읽어 gap lock이 겹치고 두 트랜잭션이 교착했다. 가입 경로의 선행 조회도 같은 종류의 gap lock을 걸어서 함께 없앴다(유일 인덱스가 중복을 판정). 이제 upsert를 먼저 하고 잠긴 행을 비교하며, 충돌이면 오류로 롤백한다. 수정 뒤 같은 시험을 연속 8회 돌려 8회 통과했다(수정 전 같은 방식으로 3회 중 1회 실패). 같은 시험은 해시 동시 4개 제한에서 6개 요청 중 일부가 429를 받는 것도 드러내 요청을 최대 5초 줄 세우도록 바꿨다. 회귀: `mysql_adoption_flow`(2)·`mysql_facereview_reads`(1)가 통과했고 `mysql_worker_write_e2e`는 스크립트가 주는 `AIP_E2E_FIXTURE_MARKER`가 없어 시작 전에 거부되어 CI 스크립트의 결과로 본다.

**Linux CI(c4e55aa).** 같은 날 push한 `c4e55aa`에서 Verify 39584326845가 `scripts/verify-mysql-worker-e2e.sh run-ci`의 Bubblewrap 안에서 이 인증 시험을 실행해 `1 passed`, Database contracts 39584326930이 세션 저장소를 MySQL 8.4와 MariaDB 11.4에서 각 4건 통과했다. 이 SHA는 위 동시성 수정 전이므로 수정 뒤 SHA의 결과는 아래 최종 기록을 따른다. 같은 SHA의 Dependencies는 RUSTSEC-2026-0381·0382·0383(rustls-webpki·webpki 루트)으로 실패했고, 이는 이 작업과 무관한 새 공개 권고였으며 `Cargo.lock` 7개를 `rustls-webpki 0.103.16`·`webpki-roots`·`webpki-root-certs 1.0.10`으로 올려 `tools/dependency-check.sh`가 `Dependency check passed.`로 통과했다.

**최종 Linux CI(2026-10-21).** 동시성 수정 `673ee0e`에서 Database contracts와 Deployment checks가 success다. 그 뒤 의존성 갱신 `30ad7b2`(코드는 같고 `Cargo.lock`만 다름)에서 Verify·Linux worker jobs·Linux cgroup v2·Object storage·External brokers·App packages·Dependencies 7종이 모두 completed/success다. Verify에서 Bubblewrap 안의 Node·Python worker 쓰기 E2E와 이 인증 E2E(`mysql_facereview_auth`)가 각각 `1 passed`로 끝났다. Database contracts와 Deployment checks는 경로 필터 때문에 `30ad7b2`에서는 실행되지 않아 `673ee0e`의 결과를 쓴다.

**알려진 한계·미구현.** 다음은 하지 않았다. (0) 같은 refresh token을 두 탭이 동시에 쓰면 하나가 이기고 다른 하나는 replay로 보여 세션이 폐기된다(유예 창 없음). (1) 로그인 시도 제한: 원본에도 없고 relational adapter는 rate limit이 없어 앞단 proxy가 맡아야 한다. 느린 해시는 동시 4개로 제한하고 넘으면 429 `WIRE.RATE_LIMITED`다. (2) 성공 로그인 때 약한 해시를 새 해시로 바꾸는 재해시(`rehash` 판정은 이미 반환). (3) 이메일 인증·비밀번호 변경·탈퇴 흐름과 `/me`·`/tutorial`(읽기와 전이 정의로 옮길 부분은 7.2.8). (4) 로그인 실패가 계정 존재를 알려 주던 원본의 A003/A004 구분은 일부러 합쳤다(`invalid_credentials`). 화면의 문구 분기는 7.2.10에서 맞춘다. 다만 `/auth/check-email`은 원본처럼 존재 여부를 공개한다. (5) 응답을 잃은 정상 클라이언트가 이미 쓴 refresh token을 다시 보내면 세션이 폐기되어 재로그인이 필요하다(유예 창 없음). (6) 접근 토큰마다 세션 표를 한 번 더 조회하는 비용을 측정하지 않았다. (7) 브라우저에서 `SameSite=None; Secure` 쿠키가 실제 교차 사이트 HTTPS 흐름에서 도는지는 실행하지 않았다(7.2.10).

## 2026-10-21 MySQL 관리 파일 저장소 (WBS 7.2.1.3, Linux CI 통과)

같은 Run 소유 컨테이너(MySQL 8.4 loopback 34411, 표준 fixture의 MySQL 8.4 34421·MariaDB 11.4 34422)에서 실행했다. 저장소 시험 `mysql_jobs`(aip-migrate) `mysql_file_store_is_owner_scoped_idempotent_and_expires`가 MySQL·MariaDB 양쪽에서 통과했다(같은 파일 6개 `6 passed`). 같은 이름·바이트·형식·만료의 재전송은 같은 핸들, 다른 바이트·형식·만료는 `FileConflict`, owner·bucket·object key별 이름 분리, 잘못된 이름(`..`)·bucket·형식과 1MiB 초과 거부, 1MiB 정확히는 저장, 다른 owner·다른 bucket의 읽기는 비어 있음, 삭제는 소유자만, 만료된 파일은 읽히지 않고 사용량에서 빠지며 정리가 지우는 것, 이진 바이트(0과 255 포함) 보존, 8개 동시 쓰기에서 같은 본문은 한 파일로 수렴하고 같은 이름의 서로 다른 본문은 정확히 하나만 이기는 것을 확인했다.

서비스 시험 `mysql_jobs::mysql_job_stores_tests::mysql_managed_files_are_put_and_read_by_jobs_and_clients_under_quota`는 MySQL 8.4에서 1 passed다. job 호스트의 put 재생(같은 effect는 같은 파일, 다른 effect는 같은 이름이어도 다른 파일, 같은 키의 다른 본문 `IDEMPOTENCY_MISMATCH`), get(장부 없이 읽기, 모르는 id `NOT_FOUND`), 미선언 bucket `ACCESS_NOT_DECLARED`, 잘못된 action `BAD_REQUEST`, 잘못된 이름·base64 `BAD_VALUE`, 800KB `PAYLOAD_TOO_LARGE`, 임차권 상실 `LEASE_LOST`와, 클라이언트 경로(job이 넣은 파일 읽기, 올리기, 같은 바이트 재전송은 같은 파일, 다른 본문 충돌, 선언 없는 bucket `NOT_EXPOSED`, 본문 형태 오류, 삭제 뒤 `NOT_FOUND`, 잘못된 id `BAD_VALUE`)을 확인한다. 100개를 채운 owner는 새 이름이 `QUOTA_EXCEEDED`다. 실제 worker E2E(`mysql_worker_write_e2e`)에 `fileJob`을 추가했다. Node·Python 양쪽에서 첫 시도가 재시도를 요청하고 두 번째 시도가 같은 put을 재생해 표에는 1건이며 worker가 `ctx.files.get`으로 읽어 내용이 맞고, HTTP `/files/get`이 `aGVsbG8=`를 주고 `/files/put`·`/files/delete`가 동작해 `1 passed`다. 같은 컨테이너에서 기존 시험도 통과했다(이벤트·메시지·메시지 핸들러 서비스 시험, HTTP effect, 트리거, `mysql_read_http` 1, `mysql_write_http` 9, 저장소 `mysql_jobs` 양 엔진, fence 8·drift 1·capability gate 4). `mysql_adoption_flow`는 "파일 업로드 설정은 거절" 단언이 이제 사실이 아니어서 처음 실패했고, 업로드 설정의 채택 계획이 성공하는 단언으로 바꿔 `2 passed`다. `cargo fmt --check`와 mysql feature `clippy --all-targets -D warnings`는 통과했다. Linux CI는 main `a0daf81`의 Verify 39577769052(실제 Node·Python worker E2E 포함), Database contracts 39577768890, Linux worker jobs 39577768904 등 8종이 모두 success다. 이 실행이 이벤트·메시지·메시지 핸들러·관리 파일 시험을 MySQL 8.4와 MariaDB 11.4 CI 컨테이너에서, worker E2E를 Linux Bubblewrap에서 돌렸다. 미실행: `file_mounts`·`/files/mount-get`, `enable_file_uploads`를 끈 서비스의 `/files/put`(`NOT_EXPOSED` 분기)의 실DB 시험, 바이트 합계 10MiB 한도의 실DB 시험(개수 한도만 시험했다), 만료 파일 정리 루프(저장소의 `cleanup_expired`만 시험했다).

## 2026-10-21 MySQL 메시지 핸들러 (WBS 7.2.1.3, Linux CI 통과)

같은 Run 소유 컨테이너에서 실행했다. 서비스 시험 `mysql_jobs::mysql_job_stores_tests::mysql_message_handlers_turn_messages_into_jobs_and_ack_with_the_acceptance`가 MySQL 8.4에서 1 passed다. 바인딩이 없는 서비스 principal은 메시지를 클레임하지 않고(배달 횟수 0), 바인딩하면 자기 owner scope의 메시지가 `message:<id>` 키의 job 1개가 되며 같은 트랜잭션에서 ACKED가 된다. 다른 owner의 메시지는 소비하지 않고, job 입력의 `message`는 queue·payload·deliveries를 담는다. ack가 접수와 함께 커밋되는 것은 낡은 임차권의 접수가 `LEASE_LOST`로 롤백되고 job이 남지 않는 것으로 확인했다. 비활성 principal은 새 메시지를 가져가지 않고 준비 상태로 남긴다. 처음 구현은 접수 뒤 메시지를 다시 READY로 돌려 재전달을 흉내 내면 `IDEMPOTENCY_MISMATCH`로 nack되어 job은 1개인데 메시지가 dead letter로 갔다. 입력의 `deliveries`가 첫 접수와 달라지기 때문이며 PostgreSQL 경로의 입력 구성도 같다. 같은 키의 job이 있으면 ack하도록 고친 뒤 job은 1개로 수렴하고 메시지는 ACKED다.

실제 서버 E2E(`mysql_worker_write_e2e`)에 서명된 웹훅 `/hooks/relay`가 서비스 principal의 `relayJob`으로 메시지를 큐에 넣고 `message_handlers`의 `handleJob`이 그 메시지를 받아 SUCCEEDED가 되며 큐에 READY·CLAIMED가 남지 않는 흐름을 추가했다. Node·Python 양쪽 모두 `1 passed`이고 서버 시작 검증(`messaging` 필수, 큐를 선언한 job, 입력이 `{ message: Text }`)도 이 경로에서 통과한다. 같은 컨테이너와 MySQL·MariaDB fixture에서 기존 시험도 모두 통과했다(저장소 `mysql_jobs` 5개 양 엔진, 이벤트·메시지 서비스, HTTP effect, 트리거, `mysql_adoption_flow` 2, `mysql_read_http` 1, `mysql_write_http` 9, fence 8·drift 1·capability gate 4). `cargo fmt --check`와 mysql feature `clippy --all-targets -D warnings`는 통과했다. Linux CI는 main a0daf81의 8종 success로 확인했다(Verify 39577769052, Database contracts 39577768890). 미실행: 명시 `owner`를 가진 핸들러, 접수 거부 뒤 nack·dead letter 경로의 실DB 시험, 만료 메시지 정리 루프(저장소의 `cleanup_expired`만 시험했고 supervisor의 주기 호출은 시험하지 않았다), PostgreSQL 경로의 같은 결함 재현.

## 2026-10-21 MySQL job 메시지 큐 (WBS 7.2.1.3, Linux CI 통과)

Run 소유 컨테이너(macOS 호스트, `--rm`·tmpfs, MySQL 8.4 loopback 34411, 표준 fixture 스크립트의 MySQL 8.4 34421·MariaDB 11.4 34422)에서 실행했다. 저장소 시험 `mysql_jobs`(aip-migrate) `mysql_message_queue_leases_retries_dead_letters_and_expires`가 MySQL·MariaDB 양쪽에서 통과했고(같은 파일 5개 `5 passed`, MariaDB 실행 포함) MySQL에서 6회 반복해도 모두 통과했다. 같은 키·본문의 같은 메시지, 다른 본문 충돌, 큐·owner 분리, 잘못된 큐 이름과 600KB 본문 거부, 지연 메시지는 아직 클레임되지 않음, 위조 토큰·다른 worker·두 번째 ack 거부, nack 재시도와 배달 횟수 소진 뒤 dead letter, owner별 dead letter 분리, redrive는 DEAD에서만 동작하고 원본 본문 유지, 만료 임차권은 새 클레임 때 에포크가 오르고 옛 임차권은 ack 못 하며 예산을 다 쓰면 `LEASE_EXPIRED`로 DEAD, dead letter 키셋 페이지, 만료 정리 배치, 8개 동시 소비자가 40건을 한 건도 겹치지 않게 가져가는 것을 확인했다.

서비스 시험 `mysql_jobs::mysql_job_stores_tests::mysql_message_queues_publish_from_jobs_and_serve_the_client_routes`는 MySQL 8.4에서 1 passed다. job 호스트의 발행 재생(같은 effect는 같은 메시지, 다른 본문 `IDEMPOTENCY_MISMATCH`, 미선언 큐 `ACCESS_NOT_DECLARED`, 키 없음 `BAD_REQUEST`, 600KB `PAYLOAD_TOO_LARGE`, 임차권 상실 `LEASE_LOST`)과 클라이언트 경로(job이 발행한 메시지 클레임, 위조 토큰 ack 거부, ack는 한 번만, 클라이언트 발행, nack, dead letter 조회, redrive, 선언 없는 큐 `NOT_EXPOSED`, 본문 형태·범위 오류, 모르는 경로 `NOT_FOUND`)을 확인한다. 1000건을 채운 owner는 새 키가 `QUOTA_EXCEEDED`이고 이미 있는 키의 재전송은 통과한다. 실제 worker E2E(`mysql_worker_write_e2e`)에 `messageJob`을 추가했다. Node·Python 양쪽에서 첫 시도가 재시도를 요청하고 두 번째 시도가 같은 발행을 재생해 큐에는 1건이며, HTTP `/messages/claim`이 그 payload를 주고 `/messages/ack`가 true이며 다음 클레임은 비어 `1 passed`다. 같은 컨테이너에서 기존 시험도 통과했다(이벤트 서비스·저장소, HTTP effect, 트리거, `mysql_adoption_flow` 2, `mysql_read_http` 1, `mysql_write_http` 9, fence 8·drift 1·capability gate 4).

`cargo fmt --check`와 mysql feature `clippy --all-targets -D warnings`(aip-migrate·aip-service·aip-cli, spike-v6-transport)는 통과했다. Linux CI는 main a0daf81의 8종 success로 확인했다(Verify 39577769052, Database contracts 39577768890). 미실행: `message_handlers`와 만료 메시지 자동 정리 루프, `messaging` 설정을 끈 서비스의 `/messages/*`(`NOT_EXPOSED` 분기)와 `client_publish`·`client_consume` 개별 차단의 실DB 시험(코드만 있다).

## 2026-10-21 MySQL job 이벤트 스트림 (WBS 7.2.1.3, Linux CI 통과)

Run 소유 컨테이너(macOS 호스트, `--rm`·tmpfs, MySQL 8.4 loopback 34411, 표준 fixture 스크립트의 MySQL 8.4 34421·MariaDB 11.4 34422)에서 실행했다. 저장소 시험 `mysql_jobs`(aip-migrate) `mysql_event_streams_publish_once_replay_in_order_and_expire_explicitly`가 MySQL·MariaDB 양쪽에서 4 passed(같은 파일의 기존 3개 포함)다. 같은 키·본문의 cursor 수렴, 다른 본문 충돌, owner·스트림 분리, 잘못된 이름·70KB 본문 거부, 페이지 재개, head 너머 cursor 거부, prune 뒤 `CursorExpired`와 남는 중복 방지 장부, 8개 동시 발행자 80건+공유 키 1건이 1..81 cursor로 빈틈 없이 쌓이는 것을 확인했다. 처음 구현은 동시 발행에서 교착(errno 1213)이 났다. `INSERT IGNORE`가 중복 행에 공유 잠금을 잡고 둘이 `FOR UPDATE`로 올리다 순환했고, `ON DUPLICATE KEY UPDATE head=head`로 바로 배타 잠금을 잡게 고쳤다. 처음에는 새 표 이름 `aip_events`가 부트스트랩의 쓰기 펜스 표와 겹쳐 `NOT_AVAILABLE`이 났고 `aip_event_log`로 바꿨다.

서비스 시험 `mysql_jobs::mysql_job_stores_tests::mysql_event_streams_publish_once_replay_in_order_and_expire_explicitly`는 MySQL 8.4에서 1 passed다. job 호스트의 발행 재생(같은 effect는 같은 cursor, 다른 본문 `IDEMPOTENCY_MISMATCH`, 미선언 스트림 `ACCESS_NOT_DECLARED`, 키 없음 `BAD_REQUEST`, 70KB `EVENT_INVALID`, 임차권 상실 `LEASE_LOST`)과 `/events/read`(자기 스트림 읽기, 선언 없는 스트림 `NOT_EXPOSED`, 잘못된 cursor `BAD_VALUE`, 필드 부족 `BAD_REQUEST`)를 확인한다. 실제 worker E2E(`mysql_worker_write_e2e`)에 `eventJob`을 추가했다. Node·Python 양쪽에서 첫 시도가 재시도를 요청하고 두 번째 시도가 같은 발행을 재생해 `aip_event_log`는 1건이며 `/events/read`가 그 payload를 돌려주고 `1 passed`다. 기존 시험도 같은 컨테이너에서 통과했다(`mysql_adoption_flow` 2, `mysql_read_http` 1, `mysql_write_http` 9, 기존 job 서비스 시험 3, fence 8·drift 1·capability gate 4).

outbox relay 시험이 12회 중 1회 `errno 1062`로 실패하는 기존 경쟁을 발견했다. READ COMMITTED에서 다른 relay가 막 커밋한 행을 다시 잡아 영수증 삽입이 중복 키가 된다. 영수증을 `INSERT IGNORE`로 넣고 실제 삽입 행만 세게 고쳤고 같은 시험이 40회 연속 통과했다(고치기 전 재현율은 12회 중 1회이므로 40회 통과는 완전한 증명이 아니다). `cargo fmt --check`와 mysql feature `clippy --all-targets -D warnings`(aip-migrate·aip-service·aip-cli, spike-v6-transport)는 통과했다. Linux CI는 main a0daf81의 8종 success로 확인했다(Verify 39577769052, Database contracts 39577768890). 미실행: SSE·WebSocket·`events_days` 정리의 MySQL 경로, aip-service 기본 lib 시험의 환경 의존 3건(Mongo fixture, PostgreSQL 연결)은 이 환경에 해당 서비스가 없어 실패했고 이번 변경과 무관하다.

## 2026-10-21 MySQL job 외부 HTTP capability (WBS 7.2.1.3, Linux CI 통과)

Run 소유 컨테이너(macOS 호스트, `mysql:8.4`, `--rm`·tmpfs, loopback 34411)에서 실행했다. `mysql_jobs::tests::mysql_http_effects_are_journaled_and_an_unknown_outcome_is_never_repeated`가 1 passed다. 완료된 호출의 답 재생(요청 1건 유지), 다른 입력의 `IDEMPOTENCY_MISMATCH`, 선언하지 않은 연동 `ACCESS_NOT_DECLARED`, 키 없음 `BAD_REQUEST`, 닿지 못한 연동 `DELIVERY_FAILED` 뒤 PENDING 0건, 답을 잃은 호출 `EFFECT_UNKNOWN`(UNKNOWN 1건, 재시도에도 요청 수 불변), 임차권 상실 `LEASE_LOST`를 확인했다. 설정에 없는 연동을 선언한 계약은 `MysqlJobs::new`가 거부한다. 같은 파일의 기존 두 시험(`..._preserve_owner_lease_and_transaction_boundaries`, `..._converge_on_server_minted_keys`)도 각각 1 passed다. 서비스는 프로세스당 schema 하나라서 시험을 한 프로세스에 하나씩 `--exact`로 실행해야 하고, 한 번에 묶으면 두 시험이 "프로세스당 한 schema" 오류로 실패한다.

실제 worker E2E `mysql_worker_write_e2e`에 `mailJob`을 추가했다. 첫 시도가 호출 뒤 `retryLater`를 던지고 두 번째 시도(attempt 2)가 같은 호출을 재생한다. macOS Seatbelt의 Node·Python worker 양쪽에서 loopback peer가 받은 요청은 1건이고 `aip_effects`의 SUCCEEDED는 1건이며 `1 passed`다. `fmt --check`와 mysql feature `clippy --all-targets -D warnings`는 통과했다. Database contracts에 새 시험 단계를 추가했다. Linux CI는 main bb6cc09의 Verify(실제 Node·Python E2E 포함)·Database contracts·Linux worker jobs 등 8종이 success다. 미실행: MariaDB 경로(이 시험은 MySQL 8.4 전용), outbound webhook 재전송 경로의 실제 DB 시험(코드만 있다).

## 2026-10-21 MySQL 트리거와 job 교착 수정 (WBS 7.2.1.3, Linux CI 통과)

Run 소유 컨테이너(macOS 호스트, Docker, `--rm`·tmpfs, loopback 55481/55491/55492)에서 실행했다. 서비스 단위 시험 `mysql_jobs::tests::mysql_triggers_run_as_the_service_principal_and_converge_on_server_minted_keys`는 1 passed다. 서비스 principal 접수·재전송 수렴·다른 본문 `IDEMPOTENCY_MISMATCH`, 사용자의 트리거 전용 연산·예약 키 거부(`ACCESS_DENIED`·`BAD_VALUE`), 미바인딩·비활성 principal `PRINCIPAL_UNAVAILABLE`, schedule slot당 한 job, 두 릴레이가 경쟁해도 outbox 행당 job 1·영수증 1, 다른 topic 미접촉, principal 비활성 중에는 영수증이 생기지 않고 복구 뒤 전달되는 것을 확인한다. 보호 장치를 하나씩 끄는 변이 3개(전용 연산 검사, 예약 키 검사, 영수증 기록)는 각각 해당 단언에서 실패했다.

실제 worker E2E(`mysql_worker_write_e2e`)를 서명된 webhook·schedule·outbox handler로 확장했다. macOS Seatbelt 격리의 Node·Python worker로 4회 연속 `1 passed`다. 서명 없음·틀린 서명은 401이고 job을 만들지 않으며, 올바른 전달은 202, 재전송은 같은 job으로 수렴하고 worker가 service principal로 영상 조회수를 1 올린다. outbox 12행이 영수증 12건과 `viewedJob` 12건이 되고 `tickJob`은 slot당 한 건이다. 이 값은 로컬 실행이며 Linux Bubblewrap 격리의 결과는 CI를 기준으로 한다. 시험은 서버 stderr를 비우고 출력하도록 고쳤다(읽지 않는 pipe는 서버를 막을 수 있고 실패 원인이 보이지 않았다).

E2E 반복에서 `/jobs/status`가 간헐적으로 `WIRE.DB_UNAVAILABLE`을 냈다(6회 중 2회). `innodb_print_all_deadlocks`로 교착을 확인했다. 요청 트랜잭션의 고정 쿼리 `SELECT 1 FROM (SELECT 1 FROM t LIMIT 1)`이 직렬화 격리에서 보조 인덱스 첫 레코드에 공유 락을 잡고, claim·settle·cancel UPDATE는 PRIMARY 다음 보조 인덱스 순서로 잠가 순환했다. 고정 쿼리를 `WHERE 1=0`으로 바꿨고(`pin_query`), `live_pre_mdl_pin_holds_the_metadata_lock_without_any_row_lock_at_serializable`이 고정 중 첫 행·첫 인덱스 항목의 UPDATE가 막히지 않고 ALTER는 `1205`로 막히는 것을 확인한다. 옛 쿼리로 되돌리면 이 시험은 `1205`로 실패한다. 수정 뒤 E2E 6회와 확장 E2E 4회가 모두 통과했다. 남은 교착은 claim(보조 인덱스 순서)과 settle UPDATE(PRIMARY 순서)의 저장소 내부 건으로, 희생자인 claim이 100ms 뒤 재시도한다(`mysql_job_claim_failed` 로그).

CI의 Database contracts MySQL·MariaDB 작업과 같은 명령을 두 엔진 컨테이너로 실행했다. `mysql_fence_verification` 3+5 passed(ALTER·DROP·RENAME·TRUNCATE 잠금 포함), `mysql_drift_matrix` 1, `mysql_capability_gate` 4, `mysql_jobs` MySQL 3·MariaDB 3, `mysql_read_http` 1·`mysql_write_http` 9, `mysql_adoption_flow` 2, 기존 HTTP job 시험 1이 통과했다. PostgreSQL 쪽 `triggers`·`outbox`·`trigger_guard` 6 passed로 `Schedule::due_slot` 정리와 서버 검증 분기가 기존 경로를 바꾸지 않음을 확인했다. aip-service·aip-migrate 기본 시험 141 passed, transport lib 73 passed, fmt·clippy(`-D warnings`)는 repo 절차 그대로 통과했다. 미실행: TLS 픽스처가 필요한 `live_tls_*`·`live_catalog_fence_*`·`live_mysql_catalog_*` 3건과 Mongo·Redis 픽스처가 필요한 `optional_adapter_combinations_*`(환경변수 없음, CI에도 없는 시험).

## 2026-10-20 MySQL HTTP job·effect journal 연결 (WBS 7.2.1.3, Linux CI 통과)

Codex macOS 명령 환경의 Run 전용 MySQL 8.4/MariaDB 11.4(tmpfs, loopback 34571/34572)에서 실행했다. `cargo test -p aip-service --features mysql --lib mysql_jobs::tests::mysql_job_http_and_effect_journal_preserve_owner_lease_and_transaction_boundaries -- --ignored --exact --nocapture`는 1 passed, `aip-migrate --features mysql --test mysql_jobs -- --ignored`는 양엔진 각각 3 passed, `mysql_write_http -- --ignored`는 8 passed, `mysql_adoption_flow -- --ignored`는 2 passed다. 마지막 CLI 회귀에는 기존 배포의 누락 effect journal을 acknowledged adoption 재실행으로 추가하는 경로가 포함된다. 새 worker fixture의 `aip service check`는 `ok:true`, CLI E2E의 `--no-run`도 성공했다. 후자는 실제 worker 실행 증거가 아니다.

HTTP 통합 시험은 dispatcher를 명시적으로 멈추고 실제 MySQL callback을 직접 실행한다. principal/contract/owner 거부, 키 수렴·충돌·한글 키, read/transition, effect의 같은 transaction 커밋과 epoch 재생, 거절된 전이 재생, 결과 재인가·출력 숨김, revoke·source DDL drift, 누락/무키 journal 시작 거부를 대조한다. 실제 HTTP→Node/Python job, effect 후 retry, 중첩 job, 대기/실행 취소는 Linux CLI E2E에 추가했고 [Verify 39438678105](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39438678105)(0aead5f)의 `scripts/verify-mysql-worker-e2e.sh run-ci`에서 `http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction ... ok`, `1 passed; 0 failed`로 통과했다(Node·Python 두 언어 루프, run 단위 로그 확인). 같은 SHA의 Database contracts·Object storage·Deployment checks·Linux cgroup v2도 success다. WBS는 메시지·파일·이벤트·외부 capability·trigger가 남아 아직 미완료다. 이번 컨테이너·cert 디렉터리는 각 시험 명령의 EXIT trap으로 제거했다. 원시 로그는 Run `6cb54067-06f5-4a7f-b9d1-0dce89718b73` outbox의 `mysql-jobs-http.log`, `mysql-job-store.log`, `mariadb-job-store.log`, `mysql-write-regression.log`, `mysql-adoption-regression.log`, `mysql-jobs-worker-build.log`다.

## 2026-10-20 MySQL 복합 키 경계 (WBS 7.2.1.1)

macOS Codex 명령 환경에서 Run 소유 MySQL 8.4 컨테이너(tmpfs, 자동 할당 loopback 포트)로 실행했다. 복합 FK를 일반 정수 필드로 숨긴 수동 매핑의 채택 계획 허용을 RED 1 failed로 재현했다. 수정 후 CLI 실제 실행 2 passed(기존 채택·JWT·드리프트와 새 복합 FK/PK 경계), HTTP 읽기 1 passed·쓰기 9 passed, catalog 18 passed·초안 단위 2 passed다. 새 검사는 계획/적용/초안의 명시 코드·업무 데이터 보존·부분 출력과 인프라 부재·단일 키 복원 후 정상 허용을 대조한다. 복합 UNIQUE는 거부하지 않는다. 원시 로그: Run `43571658-180b-4a19-895f-3fd7feea8256`의 `mysql-keys-{red,green,cli,http,catalog,unit}.log`. Linux CI와 전체 WBS 완료는 이 로컬 결과만으로 주장하지 않는다.

## 2026-10-19 main 9b75e89 Linux 실제 검증

[Linux worker CI 39238943605](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39238943605)가 성공했다. 새 JIT 경계/세션 복원/deadline 검사와 정본 seed 집계 2개가 통과했다. 같은 쿼리 JIT on 2,160.881ms(컴파일 2,158.173ms), off 0.270ms를 측정했고, 실제 제품 executor는 선언 2초 이내에 Welcome count=1을 반환했다. CLI lifecycle을 포함한 포팅 8 suite·메일 2개·sticky 1개가 통과했다. 7개 workflow는 success이며 전체 Verify는 이 시점 진행 중이다.

후속 암호화 metadata writer lookup과 `encryption_fence` 2개를 추가했다. 새 API 미존재의 compile red 후 compile-only clippy를 통과했으며 local DB를 실행하지 않았다. 이 primitive는 새 CI 전까지 실제 경합 검증 대기이고, 제품 resource 쓰기/인가/nonce budget/rekey 완료를 뜻하지 않는다.

## 2026-10-19 FlaskBB JIT 계측과 짧은 읽기 증분

main `473c528` [Linux worker CI 39238123662](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39238123662)가 성공했다. Mailpit 2개·sticky 1개·전체 기존 포팅 suite 8개가 통과했고, 이전 ignored pre-trace CLI upgrade도 실제 1 passed다. OTLP 9개(실제 worker/DB 3개 포함)와 업그레이드 보존·drift 검사로 WBS 5.2.3을 완료했다. 전체 F01–F17/Whoosh/메일 lifecycle 및 전체 출시 완료를 뜻하지 않는다.

동일 정본 seed/facts/Topic post_count SQL의 EXPLAIN ANALYZE에서 추정 cost 3,038,988.13, JIT on 1,223.152ms(함수 518개, JIT 1,220.835ms), off 0.221ms가 측정됐다. 이전 main b39633a의 같은 CLI 요청은 2초 statement timeout이었고 이번 재실행은 통과했다. JIT 비용은 측정했지만 이전 실패의 CPU/초기 로딩 상태를 동일하게 재현했다고 주장하지 않는다.

2초 이하의 짧은 PostgreSQL 읽기에만 transaction-local JIT off를 적용하는 증분을 추가했다. 2000/2001ms 경계, 성공/취소 뒤 session의 JIT on 복원, 기존 statement deadline 거부를 실제 DB 회귀로 검사한다. 로컬 compile-only clippy는 통과했으며 이 새 실행 설정의 실제 DB/CLI 회귀는 다음 CI 대기다. 더 긴 읽기의 운영자 설정과 MySQL 경로를 바꾸지 않는다.

아래 날짜별 대기는 당시 시점 기록이다. 최신 판정은 이 절과 해당 SHA별 실제 로그를 따른다.

## 2026-10-19 독립 CI 확정과 후속 증분

main `80f1fd9`의 CI 8개가 모두 성공했다. [Linux worker jobs 39233091978](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39233091978) 원시 로그는 암호화 listener gate 2개, 공개 create HTTP 회귀, 전체 OTLP 9개(실제 worker/DB 3개 포함), FlaskBB guard/대화 4개와 legacy 중복 보존 1개의 통과를 확인한다. [전체 Verify 39233092131](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39233092131)는 `AIP core verification passed.`와 실제 HTTP→Node/Python→MySQL 검사를 완료했다. [Dependencies 39233092041](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39233092041)는 전체 lock graph 검사 성공이며 `--locked`로 lock 수정을 금지한다. 이 범위로 7.5.5를 완료했다. 아래의 당시 CI 대기 설명은 이력이며 최신 판정은 이 절을 따른다.

후속 main `269bf7d`의 실제 Mailpit 활성화·재설정·중복 방지·수신자 인가는 통과했다. 장애 복구 시험은 endpoint 변경으로 구현 지문이 바뀌어 `WIRE.IMPLEMENTATION_CHANGED`로 거부되었다. main `b39633a`에서 제품 fence를 유지하고 같은 endpoint에 소유 프록시로 공급자만 복구하도록 바꿨다. `b39633a` Linux worker CI 39236890861에서 실제 복구 2개와 sticky DB 검사 1개가 통과했다. 원래 포팅 suite 8개 중 7개가 통과했고 CLI seed의 Welcome Topic post_count 조회만 statement timeout으로 실패했다. 같은 facts/seed/query로 JIT on/off EXPLAIN ANALYZE를 측정하도록 추가했으며 원인은 측정 전 가설로 둔다. 로컬 guard/프록시/정의 검사는 9 passed/1 Linux-only skipped, UI·메일 단위 검사는 20 passed다.

암호화 헤더 parser가 빈/미지원 scope tag와 비정규 `01`·`+1` version을 수용하는 반례를 로컬에서 재현했다. 형식 검사를 보완하고 유효한 s/r/o 및 u32 최대 version 양성 대조를 포함해 `cargo test -p aip-crypto --all-features --locked` 19 passed, 같은 패키지 all-targets/all-features clippy 통과를 확인했다. 이는 header 형식 검사이며 AEAD/AAD/키 가용성·외부 채택 안전성을 보증하지 않는다. 제품 암호화 readiness gate와 8.18.3–8.18.6 미완 판정은 유지한다.

기존 pre-trace schema의 CLI 업그레이드 시험은 `80f1fd9` 전체 Verify에서 명시적으로 ignored였다. 일반 OTLP 9개 성공으로 그 시험을 실행했다고 주장하지 않고, disposable PostgreSQL과 built CLI를 가진 Linux worker CI에 해당 exact ignored test 실행을 추가했다. 이 추가 검증의 결과는 다음 CI에서 별도로 확인한다.

## 2026-10-19 암호화 독립 검증 정정

`1418944`와 미커밋 회전 구현을 GPT가 대조했다. 8.18.3·8.18.4의 완료 표시는 반려하고 PMS를 미완으로 정정했다. 고정 키 map과 내부 `handle`을 사용한 시험은 배포 키 공급·scope별 DEK·writer fence·모든 저널 경로의 검증을 대신하지 않는다. 실제 CLI의 암호화 서비스 실행 차단은 유지한다.

- 로컬 `encryption_gate` 반례에서 process-local crypto를 설정하자 listener가 실행 차단을 우회했다(`1 failed`). 키 map 존재로 readiness gate를 해제하던 조건을 제거한 뒤 같은 listener 검사 2개가 통과했다.
- 외부 채택은 모든 행을 읽더라도 `parse_header`와 scope tag만 검사했다. 같은 헤더를 유지한 ciphertext 변조가 parser에는 수용되고 AEAD에는 거부되는 음성 대조를 추가했다. 실제 AEAD/AAD/키 가용성 검증 전에는 deployment/init/adopt/migrate를 거부한다. SQL 생성은 준비용 builder 검사로 남기며 배포 성공으로 세지 않는다.
- `cargo test -p aip-migrate --test encryption_gate --test encryption_activation_gate --locked --offline`: DB를 열지 않는 5개가 통과했다. 외부 채택·migration의 잘못된 DSN보다 암호화 readiness 거부가 먼저 발생하는지도 확인했다.
- `1418944`의 Linux Verify 39228801875는 `bundle_http`에서 비공개 `id` 입력에 `WIRE.FIELD_NOT_EXPOSED` 대신 `WIRE.ALREADY_EXISTS`를 반환해 실패했다. 암호화용 내부 id 예외가 일반 resource에 적용되지 않도록 범위를 좁혔다. 기존 HTTP 반례의 수정 후 실제 DB 결과는 새 CI에서 확인하며 이 로컬 회차의 clippy 통과로 대신하지 않는다.
- 회전·rekey의 미커밋 7개 파일은 복원 가능한 archive/patch와 SHA-256 manifest로 보존하고 CLI 연결에서 분리했다. 보존 구현에는 ciphertext에 대한 blind tag 계산, cutover 때 새 UNIQUE 없이 기존 열 제거, scope 없는 rekey scan 등 미해결 부분이 있다. 운영 기능으로 제공하지 않는다.

이 정정 이후의 Linux 전체 회귀 결과는 새 main SHA별 CI에서 별도로 판정한다. 아래의 이전 통과·대기 기록을 최신 전체 성공으로 재사용하지 않는다.

## 2026-10-19 FlaskBB 통합 완료 판정 보류

`4b8631d`의 7.5.5 완료 표시는 코드·이전 DoD 대조 후 review로 정정했다. 테스트 5곳에 DB 주소 fallback이 남아 있어 main의 strict DB guard를 재사용하도록 바꿨다. `node --test porting/flaskbb/tests/fixture-env.test.mjs`는 3 passed/0 failed이며, 명시적 DSN 없이도 import가 성공하던 수정 전 2 failed를 보존했다. 같은 guard를 사용하는 독립 suite 4곳도 DB 접근 전에 거부함을 확인했다.

독립 클라이언트의 mailbox 쌍 생성 경합·중복 거부와 기존 양방향 메시지 검사를 Linux worker jobs에 연결했다. `foreign_shared_uniqueness`는 기존 중복이 있는 외부 테이블의 채택 거부·UNIQUE 생성 실패 후 두 행과 원본 ID 보존을 검사한다. 이 새 DB 실행들은 로컬 clippy/구문 검사까지이며 CI 대기다. 기존 데이터 처리 절차는 [FlaskBB README](../porting/flaskbb/README.md)에 기록했고 자동 삭제·병합은 구현하지 않는다.

## 2026-10-19 최신 실행 범위

- WBS 8.18.2: main `11cf652`의 [Linux worker jobs 39214468985](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39214468985)가 성공했다. 진단 21개·암호화 정규화 10개·migrate gate 1개·transport gate 1개를 원시 로그에서 확인했다. 암호화 정의 정규화와 실행 차단 경계가 해당 task의 범위이며 제품 저장 통합 완료로 세지 않는다.
- WBS 8.18.3 증분: DEK metadata의 최초 생성·scope 분리·caller rollback·KEK-only rewrap·DB 제약 회귀 7개를 추가했다. 로컬의 식별자 검사 1개, execution/service/CLI(MySQL 포함)/transport/prototype clippy와 기존 버전 보존을 포함한 잠금 파일 9개 metadata 검사를 확인했다. PostgreSQL을 쓰는 나머지 6개와 실제 provision 최소권한 증분은 Linux CI 대기다. 로컬 `initdb`의 기존 공유 메모리 권한 거부를 재시도하지 않았다.
- WBS 8.3: `73cb6bd`의 [Linux worker jobs 39212001257](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39212001257)에서 `job_breaker` 2개, OTLP 9개가 통과했다. OTLP 중 실제 DB/worker 경로는 HTTP→job 2개와 Node/Python 중첩 실행 1개다. 중첩 `ctx.jobs.run`은 별도 durable row를 enqueue하지 않고 root lease 안에서 실행된다. root→middle→leaf의 고유 span·실제 부모·동일 trace/root job.id·DB run 1개·민감 canary 배제를 확인했다. 과거 아래의 "중첩 job이 enqueue한 job" 서술은 이 구현 의미와 다르며 당시 미검증 기록으로 남긴다.
- WBS 8.5: `a8eb690` Verify 39210323292의 장부 25개와 [Object storage 39210323219](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39210323219)의 실제 TLS MinIO 6개가 통과했다. 기존 무장부 객체/두 번째 보류 종료 뒤 도착한 PUT의 명시적 confirm 복구, owner 격리, 중복 계상 방지, quota 초과 관측의 보존을 확인했다. 자동 목록/이벤트 발견과 provider 하드 상한은 여전히 보장하지 않는다.
- WBS 8.16: [Database contracts 39207996436](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39207996436)의 SCRAM 최소권한 1개·password helper 10개·MySQL/MariaDB live fence 각 5개를 확인했다. 아래의 CI 대기 서술은 당시 이력이다.
- WBS 8.18.1: `a8eb690` Verify의 all-features 구간에서 crypto 17개가 Linux 통과했다. WBS 8.18.2 준비 단계는 로컬 parser 164개·진단 21개·암호화 정규화 10개와 runtime/migrate gate 각 1개가 통과했다(164개에 진단/정규화 포함). 5형식 동일 facts/digest, Text/Email/Url/Decimal·owner/ref metadata, 실제 CLI check/gen, TS/Python binding 동일 지문과 암호 metadata 비노출을 확인했다. 개발 검사 옵션과 관계없이 서비스 기동과 migration/adopt는 암호화 metadata를 거부한다. 이는 실제 셀 저장·읽기·회전·저널 통합 완료가 아니다.
- `a8eb690` 전체 Verify는 `job_breaker`의 두 테스트가 한 프로세스에서 다른 schema 이름을 써 실패했다. `62ac64c`에서 같은 이름을 재사용하도록 고쳤고 위 Linux 작업에서 두 테스트 모두 통과했다. 해당 전체 Verify를 성공으로 세지 않는다. 최신 전체 회귀는 별도로 확인한다.

## 포팅 공통 HTTP 전송 전 실패 분류 (2026-10-19)

- 닫힌 loopback 포트 연결이 `Transport`가 되어 durable effect를 UNKNOWN으로 남기는 반례를 재현했다. 자동 retry·redirect·환경 proxy를 끈 broker의 `send`에서 `is_connect()`인 오류만 `NotSent`로 분리한다. durable host는 `WIRE.DELIVERY_FAILED`와 intent 제거로 응답한다. 연결 이후 전송/응답 손실은 불확정을 유지한다.
- 실제 연결 거부와 **전체 요청 본문 수신 후 응답 손실**을 구분하는 regression을 포함해 `aip-execution --test integrations` **11 passed**. execution 및 transport의 관련 all-targets clippy 통과. Node/Python job의 같은 run redrive·효과 key 재생과 수신 후 UNKNOWN 유지 회귀를 추가했으며 그 실DB 실행은 다음 Linux CI에서 확인한다. 로컬 DB 실행으로 합산하지 않는다.

## WBS 8.16 최소 권한 fixture와 DB 계약 CI (2026-10-19)

- PMS의 과거 수정 완료 서술과 달리 현재 main에는 runtime role의 비밀번호 지정과 DSN password 치환이 없었다. 기존 admin 비밀번호가 유지되는 음성 대조를 재현하고 keyword/URI DSN에서 role·password를 함께 바꾸는 helper와 특수 문자·중복 query password 회귀를 추가했다. `aip-test-support` **10 passed**, 관련 CLI/helper all-targets clippy 통과.
- 최소 권한 fixture는 `LOGIN PASSWORD`와 실제 `current_database()`의 CONNECT 권한을 쓰며, SCRAM fixture에서는 틀린 비밀번호가 CLI `DB_CONNECT`로 거부되는지 확인한다. `Database contracts`에 전용 PostgreSQL 17 SCRAM fixture와 MySQL 8.4/MariaDB 11.4 live fence·drift·capability 검사를 연결했다. 실DB 결과는 CI 확인 전이므로 아직 완료로 세지 않는다.
- WBS 8.2는 main `9d2c39f`의 [Deployment checks](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39205855579) 원시 로그에서 Caddy **16/16**, OIDC/TLS **18/18**, 실제 Keycloak **12/12** 통과를 확인했다. 인증된 operation의 선언 출력 본문과 무인증·주체 revoke 거부까지 검사했다. 실제 운영 IdP 계정·운영 배포 검증은 아니다.

## WBS 8.2 IdP/TLS CI와 동시성 회귀 재현 (2026-10-19)

- `Deployment checks`에 기존 Caddy proxy 검사뿐 아니라 전체 OIDC/TLS fixture와 실제 Keycloak smoke를 연결했다. native fixture는 host loopback upstream과 소유 TLS 파일 경로를 렌더링한다. 렌더러 양성·음성 테스트 **2개 통과**, 실제 `caddy adapt` 성공, Node·shell 문법 검사 통과. 전체 DB/API 검사는 Linux CI에서 확인하며 로컬 실행으로 합산하지 않는다.
- main `b4ce7ed`의 Linux Verify는 `v3_r3`에서 생성/철회 결과 `(WIRE.CHECK_FAILED, OK)`로 실패했다. 테스트가 고정 150ms 뒤 철회를 시작해 생성 검사가 먼저 실행됐다는 보장이 없었다. 커밋의 deferred trigger를 시험용 advisory lock으로 멈추고 `pg_blocking_pids`로 생성의 검사 완료와 철회의 실제 근거 행 잠금 대기를 확인하도록 바꿨다. 제품 잠금·정책을 완화하지 않았다. 새 테스트의 실DB 실행은 Linux CI 확인 전이다.

## WBS 8.4 Python binding의 엄격 타입·출력 계약 (2026-10-19)

- `mypy --warn-return-any`에서 생성된 operation helper 7개의 `Returning Any` 오류를 재현했다. TypedDict 반환 전에 같은 공개 scalar descriptor로 실제 객체·키·null·타입·범위·ID/enum/Decimal/날짜를 검사한다. 정상·잘못된 소비자 타입 검사도 추가해 input/return 타입이 Any로 빠지는 것을 막는다. 검사기 미설치는 실패로 처리한다.
- Python **3.9.6 / 3.10.21 / 3.11.16 / 3.12.14 / 3.13.15 / 3.14.7** 각각 SDK **25개 통과**, ResourceWarning·unclosed socket 출력 없음. 각 runtime에서 생성 module import와 별도 venv의 wheel 설치·repo 밖 import를 검사했다. mypy 검사는 host Python 3.14.7 / mypy 2.4.0으로 실행했으며 각 runtime 자체에서 mypy를 돌렸다고 합산하지 않는다.
- 생성기 회귀 **9개 통과**: 실제 생성된 Python helper를 safe/decimal 두 wire에서 실행해 잘못된 출력 24종을 거부했다. 관련 SDK crate all-targets clippy와 fmt 통과. 재빌드한 CLI로 생성한 operation fixture의 mypy 출력은 `Success: no issues found in 1 source file`이다.
- 이전 package build/egg-info 산출물은 Git HEAD의 SDK와 byte 일치를 확인하고 제거했다. 새 wheel은 소유 임시 경로에서 만들고 설치·검증 후 제거한다. CI는 mypy 2.4.0을 명시적으로 설치하고 ResourceWarning을 오류로 올린다.

## WBS 8.15 OTLP 큐 드롭 계측과 제품 job 연결 (2026-10-19)

- 기존 Linux Verify `da5817e`의 원시 로그에서 `otlp.rs` **6 passed, 0 failed**와 HTTP enqueue→저장 trace→worker→collector 연결 2개를 확인했다. 이 검사는 서버가 내보낸 span을 받으며 테스트가 span을 대신 만들어 성공시키지 않는다.
- 큐 포화는 이전에 latency만 확인했다. `aip_otlp_spans_dropped_total` 누적 counter를 Prometheus와 OTLP monotonic cumulative sum에 추가했다. 로그 배치 카운터 초기화와 분리하고, 단일-thread runtime에서 5000개를 enqueue해 queue 4096개를 넘는 **904개 드롭**을 검사한다. 실제 collector 수신도 확인했다.
- 로컬 `cargo test --lib otlp::tests`: **4 passed**. queue saturation **1 passed**, 실제 OTLP collector counter 수신 **1 passed**, transport all-targets clippy `-D warnings` 통과. 이 검사는 DB를 쓰지 않는다.
- 실제 job E2E에는 collector의 `job.id`와 enqueue 결과 ID 일치 검사를 추가했다. 이 추가 단언의 Linux/DB 실행은 다음 Verify CI에서 확인하며 로컬 sandbox의 DB 실행으로 합산하지 않는다.

## WBS 8.13 prototype CLI·검증 게이트 복구 (2026-10-19)

- 현행 prototype에서 `--locked` 실패와 CLI 2개 실패를 재현했다. 공유 생성기는 `OperationBinding`을 내보내는데 prototype SDK 진입점에 이 타입이 없었다. SDK의 누락된 binding 타입을 내보내고, 이전 `PrototypeBinding` 문자열만 요구하던 검사를 현재 계약에 맞췄다. 실제 SDK를 import하는 TypeScript 호출부 컴파일 검사도 함께 통과한다.
- `cargo test --manifest-path prototype/Cargo.toml --test cli --locked --target-dir target`: **13 passed, 0 failed**. `cargo clippy --manifest-path spikes/spike-v2-read/Cargo.toml --all-targets --locked --target-dir target -- -D warnings`와 prototype fmt, `git diff --check`도 통과했다. 이 CLI 검사는 DB에 연결하지 않는다.
- prototype lock 파일에 누락된 현재 경로 의존성을 동기화하고 `tools/verify.sh`에 prototype CLI를 추가했다. 별도 workspace인 prototype도 `debug = "line-tables-only"`로 맞춰 공유 target에 전체 DWARF 빌드를 중복하지 않는다.
- 앞선 main `da5817e`의 Verify 포함 5개 CI는 성공했다. 원시 Verify 로그에서 새 PostgreSQL Bool 정렬·NULL·커서·행 정책 검사와 전체 `AIP core verification passed.`를 확인했다. 이 prototype 변경의 CI 결과와는 구분한다.

## WBS 9.3~9.5 패키지 배포와 WBS 8.14 TLS 후속 (2026-10-19)

- main `db358fa`의 [App packages](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39189959354)는 Linux x64에서 경계 검사 73개, release 두 앱 이동·SDK 소비자 검사 1개, native/Docker/복원/worker lifecycle 1개가 모두 통과했다. 원본 입력과 중간 bundle을 지운 설치 경로의 binary·정의·SDK·확장을 사용했다. 검사 harness 및 TypeScript/Python 시험 도구는 checkout에 있다.
- lifecycle은 인증 HTTP read/apply, SIGTERM 종료와 재기동, 잘못된 migration digest 거부, nullable 필드 추가와 데이터·멱등 기록 보존, 구버전 정의의 schema fence 거부, 변경 전 dump 복원 후 이전 패키지 조회·멱등 처리를 확인한다. Linux에서는 별도로 non-root/read-only/cap-drop Docker image의 build·ready·인증 조회·정상종료·재생성과 외부 DB/설정 보존, 네이티브 Node/Python 격리 worker와 durable job을 두 wire로 검사했다. 운영 배포나 Docker 안의 worker 성공을 뜻하지 않는다.
- 같은 CI의 Docker namespace 사전 probe는 `docker exec <run-owned-container> bwrap --unshare-user --unshare-net --ro-bind / / -- /bin/true`가 exit 1, `No permissions to create a new namespace`였다. 따라서 이 **기본 Docker 정책 안**의 worker 격리는 지원 조건 미충족이다. 별도 네이티브 Linux worker 실행은 통과했다. privileged·무격리로 우회하지 않았다.
- macOS arm64에서 관련 Node 검사 75 passed / 0 failed. `event-checkin`과 `folder-notes` 예제도 실제 release package 생성 → 입력 삭제 → install → 중간 bundle 삭제 → 설치 경로 check를 통과했다. macOS의 Linux 전용 Docker/worker 단계는 `NOT RUN`으로 출력한다.
- 추가 실행한 [Deployment checks](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39189659128)는 큰 본문에서 Caddy 502가 나 15/16이었다. 직접 TCP 회귀 시험도 수정 전 `ConnectionReset`으로 1/2 실패했다. 응답 전송 후 write-half FIN과 최대 200ms/2MiB의 bounded discard를 추가했다. 요청 본문 상한을 늘리거나 초과 본문을 실행하지 않는다.
- `cargo test --offline --locked --manifest-path spikes/spike-v6-transport/Cargo.toml --target-dir target --test rejected_body_close --test v8_headers`는 수정 후 2+2 passed / 0 failed, 같은 library/두 test의 clippy `-D warnings`와 fmt도 통과했다. 이 두 socket 시험은 DB를 호출하지 않으며 명시적인 비접속 시험 DSN을 guard에 전달했다. 처음 실행의 DSN 미설정 build guard 실패는 행동 RED로 세지 않았다.
- 원본 worker 설정을 잠시 제거한 로컬 Caddy 진단에서는 큰 본문 검사가 PASS였으나 SSE가 `WIRE.NOT_AVAILABLE`로 실패해 전체는 15/16이었다. 이 진단 변경은 되돌렸다. **원래 worker 설정의 최종 TLS 16개와 TCP 수정 후 전체 main CI는 재검증 전**이다. 앞선 성공을 최종 SHA 성공으로 바꾸어 적지 않는다.

## WBS 9.2 앱별 release 패키징 (2026-10-19, macOS arm64, Codex 자체 검증)

- `node --test product/tests/app-package.test.mjs product/tests/app-manifest.test.mjs product/tests/package-boundaries.test.mjs product/tests/sdk-package.test.mjs`: 58 passed, 0 failed. 앱 두 종류의 원본 제거 후 무결성, 보조 확장 파일 포함, target/feature·lockfile·경로·credential·출력 보존, 동시 입력 변경 및 실패 정리를 검사했다.
- `node --test product/tests/app-package-e2e.test.mjs`: 1 passed, 0 failed. 실제 host release 엔진으로 Event/Review 두 앱을 패키징하고, 출력 이동 및 원본 앱 삭제 후 체크섬·실제 CLI check/gen·tarball 설치·엄격한 NodeNext binding 검사·SDK import를 각각 실행했다. Python 확장도 번들 입력으로 포함했지만 이 검사는 worker 실행·HTTP·DB E2E가 아니다.
- 추가한 실패 테스트에서 내장 `.aip` 예제 변경을 감지하지 못하는 경우를 재현하고 source snapshot 범위를 보완했다. SDK 검사의 전역 npm cache 쓰기 거부는 이 Codex sandbox의 `EPERM`이며, 소유 임시 cache로 바꿔 실행·회수했다. 호스트 전체의 npm 장애로 판단하지 않았다.
- `App packages` workflow는 Ubuntu의 release 이동 검증을 추가한다. 이 로컬 기록 시점에는 원격 CI를 아직 실행하지 않았다. 운영 배포·기동 자동화(9.3)와 Docker(9.4), HTTP/worker/DB 패키지 검증(9.5)은 이 결과에 포함하지 않는다.
- SDK scratch/npm cache와 테스트 앱·번들은 각 테스트의 `finally`/종료 hook으로 제거한다. 별도 clone/worktree는 만들지 않았고 기존 엔진 `target` 한 곳을 재사용했다.

운영 IdP·TLS reverse proxy 및 실제 broker(Kafka, RabbitMQ)·S3 호환 object storage(MinIO) 연동의 로컬 컨테이너 기반 종단간(E2E) smoke 검증 기록이다. 원 작업(커밋 `cbaa401`), 독립 교차 검증(`결과.md`), 그리고 후속 IdP 원인 진단(`idp-server-config-diagnostic.md`)의 실행 결과를 시점·환경·커밋별로 병기한다. CA 코드와 문서 보완은 독립 검증 PASS다. IdP 전체 smoke는 독립 교차 검증에서 FAILED(17개 중 3개 실행, 14개 미실행)였고, 아래 WBS 8.x 통합 회차에서 원래 설정으로 17 passed였다. 후자는 자체 실행이며 독립 검증은 남아 있다. (작업 기준 부모 커밋: `4e8e2126a91bd15e21afd870fe5373c0ccba9adb`).

## WBS 8.x 통합 회차 (2026-10-17, Claude 구현·통합 / GPT 독립 검증 미실시)

Codex 작업 세션이 취소된 7개 브랜치를 이어받아 main에 통합한 회차의 기록이다. 모든 DB 의존 테스트는 이 Run이 `initdb`로 새로 만든 PostgreSQL 17.11(`127.0.0.1:55432`, DB `aip_test_claude`)에서만 실행했다. 컨테이너는 IdP·Caddy smoke에만 썼다. 이 회차의 구현과 통합은 Claude가 했고, 별도 문맥의 GPT 독립 검증은 아직 하지 않았다. 아래 PASS는 같은 Run의 자체 실행 결과이며 독립 검증을 대신하지 않는다.

| 브랜치 | 처리 | 실행 결과 |
|---|---|---|
| `codex/db-test-isolation-20261017` | 병합. 누락된 경로를 추가로 막음 | 병합 시점: 루트 workspace 402 passed, 0 failed, 5 ignored, Node 가드·DB 테스트 26 passed. 최종 트리 전체 결과는 아래 "최종 전체 실행" |
| `fix/wbs-8.1-serializable-concurrency` | 병합. 내용이 위 브랜치 버전에 포함되어 그 버전을 유지 | `absence_serializable_http` 5/5 통과, `SERIALIZATION_RETRIES=0` 변이는 3/3 실패 |
| `fix/wire-commit-unknown-20261017` (8.9) | 병합 | `commit_unknown_wire` 통과. 수정 전 `lib.rs`로 되돌리면 `WIRE.INTERNAL`(`retry: later`)로 실패 |
| `codex/wbs-8-6-cli-lint-cleanup-20261017` (8.6) | 병합 | `clippy -p aip-cli --all-targets -D warnings`가 main에서는 오류 5개 타깃(12·41·39·40·91건), 병합 후 통과. 변경 테스트 6개 타깃 통과 |
| `feature/wbs-8.4-python-sdk` (8.4) | 병합 | `python_sdk_test` 23 OK(생성 모듈 import·mypy, 깨끗한 venv wheel 설치 포함), spike-v5 46 passed, aip-cli `service` 4 passed, `service-smoke` 1 passed. 저장소의 `.aip` 21개 모두 생성되고 mypy 통과 |
| `feature/wbs-8.2-8.3-recovery-20261017` (8.2/8.3) | 일부 병합. `AIP_BINARY` 검사, `smoke-otlp.sh`, 큐 포화 테스트만 반영 | IdP·TLS smoke 원래 설정(workers 포함) 17 passed, 0 failed. OTLP 5 passed, 큐 포화 테스트 10/10 통과 |
| `fix/otlp-status-code-standard` | 병합하지 않음. 코드는 이미 main에 동일하게 있음 | `otlp.rs`·`trace.rs`는 main과 동일, 나머지 차이는 이후 가드·8.9 변경뿐. 원 브랜치의 문서 변경은 정정된 STATUS/VERIFICATION을 되돌리므로 가져오지 않음 |

### 최종 전체 실행

통합 마지막 코드 커밋 `d9ca0d8faad5672c87ddcd7423286b341eb31f54`에서 같은 Run 전용 PostgreSQL로 한 번에 실행했다. 이 뒤의 커밋은 문서만 바꾼다.

| 명령 | 결과 |
|---|---|
| `cargo test --workspace --locked --no-fail-fast` | 404 passed, 0 failed, 5 ignored |
| `cargo test --manifest-path spikes/spike-v{2-read,3-write,4-worker,5-sdk,6-transport,7-migrate,11-dev-checks}/Cargo.toml --locked --no-fail-fast` | v2 102, v3 33, v4 62(1 ignored), v5 46, v6 184, v7 1, v11 13 passed, 모두 0 failed |
| `cargo test --manifest-path prototype/Cargo.toml --locked --no-fail-fast` | 35 passed, 2 failed. 실패는 `cli`의 `gen_emits_a_typed_binding_...`, `generated_binding_resolves_...`이며 main에서도 같다 |
| `node --test product/tests/{postgres-guard,general-update,uuid-external,facereview-comments,service-smoke}.test.mjs` | 27 passed, 0 failed |
| `python3 -m unittest product.tests.python_sdk_test` | 23 tests OK |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings`, `cargo clippy -p aip-auth -p aip-migrate -p aip-service -p aip-cli --all-targets --locked -- -D warnings`, `cargo fmt --all --check` | 모두 통과 |

### 이번 회차에서 고친 문제

- DB 가드가 불완전했다. `spike_v2_read::connect()`와 `DB_URL`, `127.0.0.1:5432` 하드코딩 relay(spike v6 `db_*_lifetime`, prototype `db_startup`), 사용자 없는 proxy DSN, prototype `lifecycle`/`serve`, `product/tests/uuid-external.test.mjs`가 기본 엔드포인트를 쓰고 있었다. 모두 명시 DSN으로 바꾸고 `spikes/spike-v2-read/tests/test_database_default.rs`로 `connect()`가 DSN 없음·기본 DSN을 거부함을 고정했다.
- Node 가드는 URI query 키를 대소문자 구분으로 비교했다. 실제 libpq(`psql`)는 `HOSTADDR`·`HOST`·`SERVICE`·`SERVICEFILE` 대문자 키를 `invalid URI query parameter`로 거부하고 소문자 `hostaddr`만 적용하므로 우회 경로는 아니었다. Rust 가드와 같게 정렬하고 회귀 테스트를 추가했다. tokio-postgres의 대소문자 처리는 소스 읽기로만 확인했다.
- db-test-isolation 브랜치는 spike v2/v3/v4/v5/v7/v11과 prototype의 `Cargo.lock`을 갱신하지 않아 `--locked` 빌드가 실패했다. 갱신했다.
- CI 워크플로가 `aip_test_ci` DB를 만들지 않거나(brokers, worker-jobs-linux) DSN을 주지 않았다(deploy-checks). 맞췄다. GitHub Actions에서는 실행하지 못했다.
- `wire_codes::no_code_is_written_without_its_area`가 main(0eb0ce5)에서 이미 실패하고 있었다. metrics·OTLP의 job 상태 `RETRY`·`PENDING`·`RUNNING`이 허용 목록에 없었다. 목록에 추가했다.
- 8.9 수정 전 코드는 이미 커밋된 쓰기를 `WIRE.INTERNAL`과 `retry: later`로 돌려줬다. 응답 단절 시 `WIRE.COMMIT_UNKNOWN`으로 바꿨다.

### 사고: 공유 PostgreSQL 접속

위 가드 누락을 알기 전에 spike v2/v3/v4 전체와 spike v6 일부, prototype `db_startup`을 실행했다. 이 테스트들은 DSN에 포트가 없어 이 Mac의 공유 PostgreSQL(`127.0.0.1:5432`)에 접속했다. 대부분 자기 이름의 스키마를 만들고 지우는 테스트이고 `db_startup`은 인증만 중계했지만, 접속 사실과 중간 실패로 남은 스키마가 있는지는 확인하지 못했다. 공유 DB 접속은 금지 대상이라 확인용 접속도 하지 않았다. 인지한 즉시 실행을 멈추고 사용자에게 알림을 보냈다. `spike-v4-worker/extensions/recruitment.{mjs,py}`는 worker가 DB에 직접 닿는지 보는 격리 probe로, 5432에 TCP 연결만 시도한다. 그 probe는 의도된 fixture라 바꾸지 않았다.

### 이번 회차에도 남은 것

- 당시 `prototype` 테스트 `cli`의 2개(`gen_emits_a_typed_binding_...`, `generated_binding_resolves_...`)는 main에서도 `OperationBinding` 미export로 실패했다. **2026-10-19 후속에서 수정·13개 통과**, 위 WBS 8.13 기록 참조.
- 당시 `cargo clippy --all-targets`는 spike-v2-read의 `needless_lifetimes` 때문에 실패했다. **2026-10-19 현행 v2 all-targets `-D warnings` 통과**, 위 WBS 8.13 기록 참조.
- `prototype/Cargo.lock`은 main에서 `--locked` 검사가 이미 실패했고 이번에 갱신했다.
- 당시 8.3 큐 포화 테스트는 호출 latency만 확인했다. **2026-10-19 후속에서 누적 드롭 counter 및 collector 수신 검사를 추가**, 위 WBS 8.15 기록 참조.
- 8.9는 연결이 닫힌 경우(`is_closed`)만 `COMMIT_UNKNOWN`으로 분류한다. 그 밖의 연결 오류 형태는 실행해 보지 않았다. (다음 절 "WBS 8.9 분류 보강"에서 실행했다.)
- GitHub Actions 워크플로 변경, 실제 Kafka·RabbitMQ·MinIO smoke, Linux 격리는 이번 회차에 실행하지 않았다.

## WBS 8.9 분류 보강 (2026-10-18, Claude 구현 / 독립 검증 대기)

대상은 `spikes/spike-v6-transport`의 `/apply` 쓰기 경로다. 이 회차의 구현과 실행은 같은 Run의 Claude이고, 별도 문맥의 독립 검증은 아직 없다. DB 테스트는 이 Run이 만든 PostgreSQL 17.11 컨테이너(`127.0.0.1:32786`, DB `aip_test_wbs89`)에서만 실행했고 공유 `127.0.0.1:5432`에는 접속하지 않았다. 저장소의 DSN 가드(`aip-test-support`)가 켜진 상태다.

### 바꾼 분류

| 연결 단절 시점 | 수정 전 | 수정 후 |
|---|---|---|
| COMMIT 전송 전(UPDATE 직후, 멱등 INSERT 직후) | `WIRE.INTERNAL`, `retry: later` | `WIRE.DB_UNAVAILABLE`. 커밋되지 않았고 같은 키로 다시 보내면 새로 실행된다 |
| COMMIT 호출 시점에 연결이 이미 닫힘(COMMIT이 나가지 못함) | `WIRE.COMMIT_UNKNOWN`(표지를 먼저 올림) | `WIRE.DB_UNAVAILABLE`, 표지 유지 안 함 |
| COMMIT 전송 뒤 응답 손실(FIN, RST, 서버가 COMMIT을 받지 못함, backend 종료) | `WIRE.COMMIT_UNKNOWN` | 같다 |
| COMMIT 응답을 해석할 수 없음(`Kind::Parse`), 예상 밖 메시지(`Kind::UnexpectedMessage`) | `WIRE.INTERNAL` | `WIRE.COMMIT_UNKNOWN` |
| 서버가 거절한 COMMIT(23514, 23505, 40001, 40P01) | `BAD_VALUE`, `ALREADY_EXISTS`, `CONFLICT` | 같다. `COMMIT_UNKNOWN`으로 넓히지 않는다 |

판정은 `connection_failed`(SQLSTATE가 없고 `is_closed`이거나 Io·UnexpectedMessage·Parse 문구)이다. 서버 SQLSTATE가 있는 오류와 로컬 파라미터 직렬화 오류는 연결 오류로 보지 않는다. COMMIT 전 단계에서 쓰기 엔진이 오류 원본을 지운 거절은 연결이 이미 닫혔고 코드가 `WIRE.INTERNAL`일 때만 `DB_UNAVAILABLE`로 바꾼다.

### 실행 결과

| 명령(`spikes/spike-v6-transport`, `/tmp/aip-wbs89-classify-20261018`) | 결과 |
|---|---|
| 새 테스트를 먼저 쓰고 분류 코드는 수정 전과 같은 상태로 `cargo test --locked --test commit_unknown_wire` (red) | 11개 중 5 failed: 전송 전 단절 2개(`WIRE.INTERNAL`), 이미 닫힌 연결(`COMMIT_UNKNOWN`), 해석 불가·예상 밖 응답 2개(`WIRE.INTERNAL`) |
| 수정 후 같은 명령 (green) | 11 passed, 0 failed |
| 변이 1: `connection_failed`를 `is_closed`만으로 되돌림 | 2 failed (`unparseable_*`, `unexpected_message_*`) |
| 변이 2: COMMIT 전 `is_closed` 검사 제거 | 1 failed (`commit_on_an_already_closed_connection_*`) |
| 변이 3: 전송 전 코드를 `WIRE.INTERNAL`로 되돌림 | 3 failed (전송 전 단절 2개, 이미 닫힌 연결) |
| 변이 4: `connection_failed`를 항상 true로 넓힘 | 2 failed (`definite_commit_rejections_*`, `connection_failure_predicate_*`) |
| `cargo clippy --all-targets --locked -- -D warnings` (spike-v6) | 통과. 수정 전에는 `commit_unknown_wire.rs` 105·202·221줄의 3개(`manual_range_contains`, `bool_assert_comparison` 2개)로 실패 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` (루트) | 통과(수정 전에도 통과. spike는 루트 workspace가 아니다) |
| `cargo fmt --all --check` (루트) | 통과. 수정 전 main에서도 `spike-v6-transport/tests/otlp.rs` 포맷 차이로 실패하고 있어 그 파일만 포맷했다 |
| `cargo test --locked --no-fail-fast` (spike-v6 전체) | 187 passed, 7 targets failed(아래) |

전체 실행의 실패 7개 타깃은 수정 전 main에서도 같은 테스트가 같이 실패했다. `v12_typed`, `v13_idwire`, `v14_apply`, `v15_values`, `v16_bundle`은 `tsc`가 이 환경에 없어 파일을 찾지 못하는 오류(`NotFound`)이고, `--lib`는 `pool::tests::checkout_is_bounded_by_pool_capacity_and_acquire_deadline`(30ms 기한)이다. 이 pool 테스트는 단독 실행 3회는 통과했다. `write_isolation`은 pool checkout 기한 초과(`WIRE.DB_UNAVAILABLE`)로 실패하며 원인은 조사하지 않았다. 이 항목들은 이번 변경과 무관한 사전 실패다.

### 미검증·남은 것

- `Kind::Io`는 연결 task 안에서만 생기고 호출자에게는 `Closed`로 전달된다. RST를 보내는 테스트도 호출자에게는 `Closed`로 보였다. 따라서 `Io` 자체의 호출자 쪽 재현은 못 했다. 표시 문구 비교는 tokio-postgres 0.7.18 기준이며, 그 문구가 바뀌면 해석 불가·예상 밖 응답 테스트가 깨지도록 했다.
- 서버가 보내는 SQLSTATE 08·57P01(관리자 종료) 오류는 분류에 넣지 않았다. `pg_terminate_backend` 재현에서도 호출자는 FATAL 대신 `Closed`를 받아 그 경로를 만들 수 없었다.
- COMMIT 호출 직전의 `is_closed` 검사와 실제 전송 사이의 짧은 경합에서 닫히면 `COMMIT_UNKNOWN`(보수적 쪽)으로 보고된다.
- 이 보강은 `/apply` 핵심 경로(`apply_attempt`)만 고쳤다. 같은 COMMIT 분류가 필요할 수 있는 다른 경로는 이 절에서 실행하지 않았다(다음 절 "WBS 8.9 나머지 COMMIT 경로"에서 실행했다): `write_extension.rs`(`classify_write_error("커밋", ..)`), `spike-v4-worker/src/write.rs`, `spike-v3-write/src/lib.rs`의 COMMIT, `durable.rs`.
- COMMIT 전 단계에서 쓰기 엔진이 오류 원본을 지운 거절은 서버 FATAL(57P01 등)이면 `WIRE.INTERNAL`로 남는다.
- GPT 독립 검증 전이다. 완료가 아니라 "독립 검증 대기"다.

## WBS 8.9 나머지 COMMIT 경로와 spike-v6 사전 실패 (2026-10-18, Claude 구현 / 독립 검증 대기)

앞 절의 분류 규칙을 `write_extension.rs`, `spike-v4-worker/src/write.rs`, `spike-v3-write/src/lib.rs`, `durable.rs`에 적용하고, spike-v6의 사전 실패 세 가지를 조사했다. 이 회차의 구현과 실행은 같은 Run의 Claude이고 별도 문맥의 독립 검증은 아직 없다. DB 테스트는 이 Run이 만든 PostgreSQL 17.11 컨테이너(`aip-pg-wbs89b-1018003521-51030`, `127.0.0.1:32787`, DB `aip_test_wbs89b_1018003521_51030`)에서만 실행했고 공유 `127.0.0.1:5432`에는 접속하지 않았다. 저장소의 DSN 가드(`aip-test-support`)가 켜진 상태다. 컨테이너는 이 Run이 띄웠고 작업이 끝난 뒤 정리했다.

### spike v3·v4를 고친 근거

세 spike는 실험 코드지만 제품 경로다. `aip-service`와 `aip-auth`, `aip-cli`가 `spike-v6-transport`를 path dependency로 쓰고, v6는 다시 v3·v4를 쓴다. README도 "독립 spike의 검증된 엔진을 path dependency로 재사용한다"고 적는다. 그래서 문서화만 하지 않고 고쳤다.

### 공통 함수

a5774b3의 판정(`connection_failed`, `lost_before_commit`, 거절 재분류, COMMIT 전송과 분류)은 v6 `lib.rs`에만 있었다. 의존 방향이 v6, v4, v3 순이라 가장 아래인 `spike-v3-write`로 옮겼고 v4와 v6는 그것을 쓴다.

| 함수 | 역할 |
|---|---|
| `connection_failed(&Error)` | SQLSTATE가 없고 `is_closed`이거나 Io·UnexpectedMessage·Parse 문구인 오류 |
| `lost_before_commit()` | COMMIT 전에 연결을 잃은 실패(`WIRE.DB_UNAVAILABLE`) |
| `before_commit_rejection(&tx, Reject)` | 쓰기 엔진이 오류 원본을 지운 `WIRE.INTERNAL` 거절이 연결이 닫힌 뒤의 것이면 `DB_UNAVAILABLE`로 바꿈 |
| `send_commit(tx, on_send)` | 연결이 이미 닫혔으면 `NotSent`, COMMIT을 보낸 뒤 연결 오류면 `Unknown`, 서버가 SQLSTATE로 거절하면 `Rejected(e)`. `on_send`는 보내기 직전에만 불려 결과 불명 표지를 올린다 |

v6 `/apply`(`commit_attempt`)도 같은 `send_commit`을 쓰도록 바꿨다. 앞 절의 `commit_unknown_wire` 11개는 리팩터링 뒤에도 그대로 통과했다.

### 경로별 적용

| 경로 | COMMIT 전 연결 끊김 | COMMIT 전송 뒤 끊김·해석 불가 | 서버 거절 |
|---|---|---|---|
| v3 `apply`·`bundle`·`compose`(`commit`, 세 진입점) | `WIRE.DB_UNAVAILABLE`(수정 전 `WIRE.INTERNAL`) | `WIRE.COMMIT_UNKNOWN`(수정 전 `WIRE.INTERNAL`) | 기존 코드 유지 |
| v4 `invoke_write` | `WIRE.DB_UNAVAILABLE`. 확장이 ctx 쓰기 중 연결을 잃은 경우도 포함(수정 전 `WIRE.EXTENSION_ERROR`) | `WIRE.COMMIT_UNKNOWN`(수정 전 `WIRE.INTERNAL`) | 기존 코드 유지 |
| v6 `write_extension::apply` | `WIRE.DB_UNAVAILABLE`(수정 전 `WIRE.INTERNAL`) | `WIRE.COMMIT_UNKNOWN`(수정 전 `WIRE.INTERNAL`) | 기존 코드 유지 |
| v6 `durable.rs` 쓰기 요청 COMMIT 두 곳(job 접수·파일·메시지 handler, trigger 접수) | 같다(이미 `DB_UNAVAILABLE`). COMMIT이 나가지 못한 경우만 `DB_UNAVAILABLE`로 바꿈(수정 전 `COMMIT_UNKNOWN`) | `WIRE.COMMIT_UNKNOWN`(같다) | 기존 `COMMIT_UNKNOWN`을 유지(넓히지도 좁히지도 않음) |
| v6 `durable.rs` managed effect journal COMMIT(데이터 apply·파일·이벤트·메시지) | `WIRE.DB_UNAVAILABLE`. 이제 job이 이 코드로 재시도된다(수정 전에는 확장 오류로 `FAILED`) | `WIRE.COMMIT_UNKNOWN`으로 job 종료(수정 전 `FAILED`와 `WIRE.EXTENSION_ERROR`) | `WIRE.DB_UNAVAILABLE`(기존) |

durable.rs의 나머지 COMMIT 지점은 바꾸지 않았다. 이유는 지점별로 다르다.

- 읽기 전용이거나 principal 확인만 하는 트랜잭션(`guard`, 하트비트, 읽기 op, 파일 get, 재생 조회)과 만료 정리(GC)는 쓰기 결과를 호출자에게 보고하지 않거나 멱등이다.
- 워커 내부에서 로그만 남기는 지점(job 결과·취소·admission 기록)은 이미 오류를 모두 같은 로그로 남기며 임대 만료가 복구한다.
- HTTP effect의 journal(PENDING·UNKNOWN·SUCCEEDED 표시) COMMIT은 `status` 응답이 PENDING·UNKNOWN 행을 `WIRE.EFFECT_UNKNOWN`으로 덮어 보여 주는 별도 장치가 있다. 이 장치를 대체하지 않으려고 바꾸지 않았다.
- 자식 job·operation 결과 기록(`state=$3,result=$4`)과 업무 거절 기록(`reject_data_effect`)은 재현하지 않아 바꾸지 않았다. 후자는 데이터가 바뀌지 않는다.

managed effect journal의 변경은 제품 동작 변경이다. 두 가지가 바뀐다. 하나는 COMMIT을 보낸 뒤 응답을 잃으면 job이 확장 오류가 아니라 `WIRE.COMMIT_UNKNOWN`으로 끝나는 것이다(`FAILED`이지만 쓰기는 커밋됐을 수 있고, 자동 재시도하지 않는다). 다른 하나는 `WIRE.DB_UNAVAILABLE`을 job host가 확장에 넘기지 않고 job 시도 실패로 올려, 이미 있던 재시도 규칙(`retryable`)이 적용되는 것이다. `product/MANAGED-EXECUTION.md`의 "DB 장애는 아무것도 commit하지 않았으므로 ... 다시 시도할 수 있다"는 기존 문서 문장을 코드가 처음으로 지킨다. 확장이 `catch`로 DB 장애를 직접 처리하던 job은 이제 그 오류를 받지 못하고 시도 전체가 재시도된다. 이 동작 변경은 독립 검증에서 따로 봐야 한다.

### 테스트 도구

장애 지점마다 같은 TCP proxy가 필요해서 `crates/aip-test-fault-proxy`를 새로 만들었다. 테스트 DB 가드(`aip-test-support`)가 허용한 endpoint 앞에만 서고, 한 번만 장애를 일으킨다. 장애는 COMMIT 성공 응답 삭제(FIN·RST), COMMIT을 서버에 전달하지 않고 닫기, COMMIT 응답을 다른 메시지로 바꾸기, 지정 문장이 끝난 직후 닫기가 있고, COMMIT 단계 장애는 지정한 문장을 보낸 연결에서만 일으킬 수 있다(서버 배경 작업의 COMMIT이 장애를 먼저 쓰지 못하게 한다). 앞 절의 `commit_unknown_wire` 테스트는 자체 proxy를 그대로 쓴다.

### 실행 결과

모든 명령은 `/tmp/aip-wbs89b-20261018`(이 회차 worktree)에서 실행했다. spike 명령은 해당 `spikes/<crate>` 디렉터리에서 `cargo test --locked`다.

| 대상 | red | green |
|---|---|---|
| v3 `commit_classification`(`apply`·`bundle`·`compose`) | 14개 중 12 failed(`WIRE.INTERNAL`, 구현 전). 통과 2개는 서버 거절 코드 유지와 판정 함수 대조 | 구현 뒤 14 passed. 이후 compose 3개와 BEGIN 단절 1개를 더해 18 passed |
| v4 `commit_classification` | 9개 중 8 failed(`write.rs`를 HEAD로 되돌려 실행). 통과 1개는 서버 거절 코드 유지 | 9 passed. BEGIN 단절을 더해 10 passed |
| v6 `write_extension_commit_classification` | 10개 중 8 failed(`write_extension.rs`를 HEAD로 되돌려 실행). 통과 2개는 서버 거절 코드 유지와, v4 수정이 이미 해결한 확장 쓰기 중 단절 | 10 passed. BEGIN 단절과 확장 세션 설정 단절을 더해 12 passed |
| v6 `durable.rs` 단위 `commit_request_on_an_already_closed_connection_is_db_unavailable` | 스텁을 수정 전 동작으로 두고 1 failed(`COMMIT_UNKNOWN`) | 통과 |
| v6 `durable_commit_classification`(HTTP `/jobs/enqueue`, 4개) | 처음부터 4 passed. 수정 전 코드도 이 단절 지점들은 이미 규칙대로였고, 이 테스트는 그 동작을 고정한다 | 4 passed |
| v6 `durable_job_commit_loss`(job의 데이터 apply COMMIT 손실 2개, 전이 UPDATE 직후 단절 1개) | 3개 모두 failed(`WIRE.EXTENSION_ERROR`로 끝남. COMMIT 응답 손실 시에도 데이터는 커밋됨) | 3 passed(`COMMIT_UNKNOWN` 2개, 재시도 뒤 `SUCCEEDED`·시도 2회·쓰기 1회 1개) |
| v6 `commit_unknown_wire`(앞 절) | 리팩터링 뒤에도 11 passed | BEGIN 단절 1개를 더해 12 passed |
| 전체: v3, v4, v6 각 `cargo test --locked --no-fail-fast` | | v3 51 passed, v4 72 passed(1 ignored), v6 217 passed, 모두 0 failed |

### 변이 확인

분류를 되돌리거나 넓히는 변이를 하나씩 넣고 관련 스위트(v3·v4·v6)를 다시 실행했다. 20개 변이가 모두 실패로 잡혔다(killed). 처음 실행에서 살아남은 M9와 M12는 해당 지점을 겨냥한 테스트(v4의 `START TRANSACTION` 단절, v6 `write_extension`에서 v4가 보내는 세션 설정 문장 단절)를 더한 뒤 잡혔다. M19도 같은 방식으로 v6 `commit_unknown_wire`에 `START TRANSACTION` 단절 테스트를 더해 확인했다.

| 변이 | 잡은 스위트와 실패 수 |
|---|---|
| M1 `connection_failed`에서 문구 판정 제거 | v3 2, v4 2, v6 4 |
| M2 `send_commit`의 이미 닫힘 검사 제거 | v3 3, v4 1, v6 1, v6 lib 2 |
| M3 `send_commit`이 전송 뒤 연결 오류를 서버 거절로 취급 | v3 8, v4 5, v6 13 |
| M4 `before_commit_rejection`이 거절을 바꾸지 않음 | v3 3, v4 2, v6 2 |
| M5 `connection_failed`가 항상 참 | v3 2, v4 1, v6 4 |
| M6 v3 `begin_failure`가 항상 내부 오류 | v3 2 |
| M7 v4 ctx 쓰기 연결 단절을 바꾸지 않음 | v4 1, v6 1 |
| M8 v4 `invoke_write`의 거절 재분류 제거 | v4 1 |
| M9 v4 `invoke_write`의 트랜잭션 시작 단절을 분류하지 않음 | v4 1 |
| M10 v6 `write_extension`의 COMMIT 전 오류가 항상 `INTERNAL` | v6 4 |
| M11 v6 `write_extension`이 전송 뒤 불명을 `DB_UNAVAILABLE`로 냄 | v6 5 |
| M12 v6 `write_extension`이 준비 단계 거절을 재분류하지 않음 | v6 1 |
| M13 v6 `commit_attempt`의 `NotSent`를 `COMMIT_UNKNOWN`으로 냄 | v6 1 |
| M14 durable 쓰기 요청이 전송 뒤 불명을 `DB_UNAVAILABLE`로 냄 | v6 3 |
| M15 durable 쓰기 요청이 `NotSent`를 `COMMIT_UNKNOWN`으로 냄 | v6 lib 1 |
| M16 durable effect가 전송 뒤 불명을 `DB_UNAVAILABLE`로 냄 | v6 2 |
| M17 durable effect가 `NotSent`를 `COMMIT_UNKNOWN`으로 냄 | v6 lib 1 |
| M18 v4 job host가 `COMMIT_UNKNOWN`을 넘기지 않음 | v6 2 |
| M19 v6 `/apply`의 트랜잭션 시작 단절을 분류하지 않음 | v6 1 |
| M20 v4 job host가 `DB_UNAVAILABLE`을 넘기지 않음 | v6 1 |

### 검사 명령

| 명령 | 결과 |
|---|---|
| `cargo fmt --all --check` (루트) | 통과 |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings` | 통과 |
| `cargo clippy -p aip-auth -p aip-migrate -p aip-service -p aip-test-fault-proxy -p aip-test-support --all-targets --locked -- -D warnings` | 통과 |
| `cargo clippy -p aip-cli --features poc-runtime --bins --locked -- -D warnings` | 통과 |
| `cargo clippy --all-targets --locked -- -D warnings` (spike-v6) | 통과 |
| `cargo clippy --lib --test commit_classification --locked -- -D warnings` (spike-v3, spike-v4) | 각각 통과 |
| `cargo clippy --all-targets --locked -- -D warnings` (spike-v3, spike-v4) | 실패. main(`a5774b3`)에서도 같다. v3는 `general_update` 테스트의 참조 오류 1건, v4는 lib 테스트의 `marker_processes` 미사용과 길이 비교 오류 2건이며 이번 변경 파일이 아니다 |
| `cargo test --workspace --locked --no-fail-fast` | 405 passed, 0 failed, 5 ignored |
| `cargo test -p aip-test-support --features strict-db --locked` | 7 passed, 0 failed |
| `cargo test --locked --no-fail-fast` (spike-v3, spike-v4, spike-v6) | v3 51 passed, v4 72 passed(1 ignored), v6 217 passed, 모두 0 failed |

워크스페이스 전체 실행에서 `aip-cli`의 `least_privilege`가 처음에 실패했다(`DB_CONNECT`). 이 Run의 컨테이너가 비밀번호 인증(`scram-sha-256`)만 받아 비밀번호 없는 `CREATE ROLE ... LOGIN` 역할이 접속하지 못한 것이고, 코드 변경 전 main(`a5774b3`)에서도 같은 실패였다. 이 Run 전용 컨테이너의 `pg_hba.conf`를 `trust`로 바꿔 다시 실행했더니 통과했고(위 405 passed), 공유 DB는 건드리지 않았다.


### spike-v6 사전 실패 조사

| 항목 | 판단 | 조치와 결과 |
|---|---|---|
| `write_isolation` | 제품 결함이 아니라 테스트 지원 함수 결함이다. `aip_test_support::database_url_with_option`이 URI의 option 값을 form 인코딩(`append_pair`)해 공백을 `+`로 썼다. libpq와 tokio-postgres는 URI query의 `+`를 공백으로 읽지 않아 서버가 `+default_transaction_isolation`이라는 이름의 설정으로 받고 `unrecognized configuration parameter`로 접속을 거부했다. 서버 pool이 연결을 못 얻어 `WIRE.DB_UNAVAILABLE`(pool checkout 오류)이 났다. `postgres://` 형식 DSN에서만 생기고 키워드 형식 DSN은 영향이 없다 | 값을 퍼센트 인코딩(공백은 `%20`)으로 쓰고 기존 query를 유지하도록 고쳤다. 단위 테스트(`uri_option_values_encode_spaces_as_percent_20`)는 수정 전 코드에서 failed, 수정 뒤 passed. `write_isolation` 1 passed. 같은 함수를 쓰는 `aip-migrate`의 `migration_history`도 main에서 2 failed(`DB_CONNECT`)였고 수정 뒤 3 passed |
| pool 30ms 타임아웃 | 시간에 민감한 불안정 테스트다. `acquire_timeout`이 30ms인 pool에서 첫 `acquire()`가 새 연결을 만들면 연결 시간이 같은 30ms 기한을 쓴다. 부하가 있으면 첫 호출이 `Timeout`으로 먼저 실패했다(`pool.rs:237`의 `unwrap`). 두 번째 호출은 슬롯이 잡혀 있어 성공할 수 없으니 이 시간 의존은 불필요하다 | 연결을 idle 목록에 미리 넣어 첫 checkout이 연결을 만들지 않게 하고 두 번째 호출만 30ms 기한을 받게 했다. 최소 대기 시간 단언을 더했다. spike-v6 lib 테스트 전체를 연속 실행해 수정 전 8회 중 4회 실패, 수정 뒤 12회 중 0회 실패였다(둘 다 같은 부하의 다른 변이 실행이 겹친 상태) |
| v12~v16 `tsc` 부재 | 환경 문제다. 이 테스트들은 `spikes/spike-0-ts/node_modules`의 `tsc`를 쓰는데 그 디렉터리는 추적되지 않는 설치물이라 새 worktree에는 없다. 주 checkout에는 있다 | 전역 설치는 하지 않았다. 저장소가 정한 경로 `npm ci --prefix spikes/spike-0-ts --ignore-scripts`로 설치(typescript 7.0.2)한 뒤 `v12_typed`, `v13_idwire`, `v14_apply`, `v15_values`, `v16_bundle` 5개가 모두 passed. 코드 변경은 없다 |

### 미검증·남은 것

- `Kind::Io`의 호출자 쪽 직접 재현, 서버가 보내는 SQLSTATE 08·57P01 오류의 분류는 앞 절과 같이 미실행이다.
- `write_extension.rs`와 v4 `invoke_write`의 "COMMIT 호출 직전에 이미 닫힘" 검사는 `send_commit` 하나로 합쳤고, 그 분기는 v3의 세 경로(일시 정지 knob과 `pg_terminate_backend`), v4, v6 `commit_attempt`, durable 단위 테스트가 본다. `write_extension` 경로에서 이 분기에만 닿는 재현은 만들지 못했다(확장 뒤 멱등 INSERT가 먼저 닫힌 연결을 만난다).
- `durable.rs`의 자식 job 결과 기록과 업무 거절 기록, GC·워커 내부 지점은 실행하지 않았다. managed effect journal에서 COMMIT 응답을 잃은 job을 운영자가 어떻게 확정하는지(명령)는 아직 없다.
- 서버 거절 COMMIT(제약 위반·직렬화 실패)을 `durable.rs`의 쓰기 요청 경로가 `COMMIT_UNKNOWN`으로 내는 것은 기존 동작을 유지한 것이다. 확정 rollback인데 결과 불명으로 보고하는 보수적 분류이며, 바꿀지는 정하지 않았다.
- managed effect journal의 job host 변경(`DB_UNAVAILABLE` 넘김, `COMMIT_UNKNOWN` 종료)은 제품 동작 변경이다. 이 변경이 다른 job 시나리오(HTTP effect, 중첩 job, 메시지)에 미치는 영향은 v6 전체 스위트가 통과한 범위까지만 확인했고 운영 환경은 미실행이다.
- 다른 crate의 직접 COMMIT(`aip-runtime`, `aip-execution`, `aip-migrate`, `aip-service`의 `tx.commit()`)은 이번 범위가 아니라 분류를 확인하지 않았다. 대부분 `map_err(|_| ...)`로 오류를 종류와 상관없이 자기 코드(`COMMIT_UNSETTLED`, `JOBS_UNSETTLED` 등)나 `WIRE.COMMIT_UNKNOWN`으로 한 번에 내므로, COMMIT 전 단절을 `DB_UNAVAILABLE`로 가르는 규칙이 필요한지는 별도로 봐야 한다.
- GPT 독립 검증 전이다. 완료가 아니라 "독립 검증 대기"다.

## WBS 8.10 독립 검증 결함 6건 수정 (2026-10-18, Claude 구현 / 독립 검증 대기)

GPT 독립 검증(기준 HEAD `7585ec5`)이 찾은 결함이다. 상세 재현 보고서는 가드가 읽기를 막아 읽지 못했고 요청에 적힌 요약으로 작업했다. DB는 이 Run의 PostgreSQL 17 컨테이너(`127.0.0.1:32801`)만 썼고, Keycloak·MinIO·Caddy는 이 Run이 띄운 컨테이너다. 모두 같은 Run의 구현과 실행이며 독립 검증 전이다.

| # | 결함 | 수정 | 증거 | 커밋 |
|---|---|---|---|---|
| 1 | 실제 Keycloak 26.2.5 토큰에는 `nbf`가 없어 거부됨 | `nbf`를 선택 claim으로 바꿈. 있으면 여전히 강제하고(미래 값, 음수, null, 문자열은 거부) `iat` 필수·미래 `iat` 거부·900초 상한은 그대로다. 근거: RFC 7519 선택 claim, RFC 9068 필수 목록에 없음, `iat`가 사용 시작을 이미 묶음. runbook 갱신 | 실제 Keycloak 컨테이너 smoke(`scripts/smoke-keycloak.sh`, TLS와 HTTPS JWKS, `aud` 배열, 암호화 키가 섞인 JWKS): 수정 전 12개 중 2 FAIL(세션과 operation), 수정 뒤 12개 모두 PASS. `cargo test -p aip-auth` 15·2·11 passed | `3b0925f` |
| 2 | IdP smoke의 17번째 확인이 HTTP 200만 보고 본문 `WIRE.BAD_REQUEST`를 놓침 | 선언된 operation(`length`)을 호출해 본문 `ok:true`와 `output.length===3`을 검사하고, 토큰 없는 같은 호출이 본문에서 `WIRE.UNAUTHENTICATED`인지 검사 | `scripts/smoke-idp-proxy.sh`: `[SUCCESS] All 18 checks passed!` | `99e1052` |
| 3 | Python 코드젠이 `typing.Never`·`NotRequired`(3.11+)와 import 시 평가되는 `A \| B`·`X \| None`(3.10+)을 써 3.9/3.10에서 ImportError | `NoReturn`, `Union`, `Optional`, 버전 분기한 `NotRequired`(3.11 미만은 런타임 stand-in, 정적 검사는 `typing_extensions`). 소스 검사 시험 추가(수정 전 코드에서 실패) | `uv run --python 3.9`(3.9.6)·`3.10`(3.10.21)·`3.13`(3.13.15)에서 `python -m unittest product.tests.python_sdk_test` 각각 `Ran 23 tests ... OK`. 처음 두 시도는 3.9에서 실제로 실패해 `Union`·`Optional`까지 넓혔다 | `5b7f23c` |
| 4 | trace E2E가 손으로 만든 span이라 동어반복이고 job span에 부모가 없음 | 요청 span id를 요청 시작 때 고정하고 enqueue 때 trace와 함께 기존 `trace_id` text 컬럼에 `<trace>-<span>`으로 저장(스키마 변경 없음, trace만 적힌 기존 행은 그대로 읽힘). job span의 `parentSpanId`가 그 요청 span이다. trace id fallback이 span 크기(16자) id를 쓰던 것도 고침. 손으로 span을 내보내는 시험을 지우고 HTTP enqueue부터 서버가 내보낸 span을 읽는 시험 2개로 교체 | `cargo test --test otlp`(spike-v6): 6 passed. 새 시험 2개는 이전 코드에서 `parentSpanId` 없음으로 실패. spike-v6 전체 227 passed·0 failed, `aip-execution` 124 passed | `dc95dc9` |
| 5 | `cargo clippy --features poc-runtime --all-targets` 14개 타깃 실패 | 8곳(불필요한 borrow 3, `unwrap` 3, 미사용 변수 2)을 고쳐 `tools/verify.sh`의 clippy 단계를 `--all-targets`로 올림 | `cargo clippy -p aip-cli --features poc-runtime --all-targets --locked -- -D warnings` 통과, 고친 e2e 5개 모음 4+9+15+1+4 passed | `791367b` |
| 6 | `prototype/tools/*.py` 기본 DSN이 `host=localhost dbname=postgres` | 공통 가드(`dedicated_db.py`)가 명시적 host·고포트(5432 제외)·user·`aip_test_*` DB, `PG*` 변수 미설정을 요구. 기본값 없이 `--db-url` 또는 `AIP_TEST_DATABASE_URL`만 받음. 묶음 README 예시도 전용 DSN 변수로 변경 | `python3 -m unittest product.tests.dedicated_db_test` 5 OK(13개 거부 DSN 포함). 인자 없이 실행 시 `refusing PostgreSQL target: no DSN given`, 옛 기본값은 `the port must be explicit` | `a8b0d0b` |

### 확인하지 못했거나 남은 것

- 3의 3.11·3.12는 실행하지 않았다(3.9·3.10·3.13만). 3.9/3.10에서 `NotRequired` 키는 런타임에서 선택 여부를 구분하지 못한다(정적 검사에서만).
- 4의 중첩 job(job이 enqueue한 job)이 부모를 이어받는 경로는 코드로 연결했지만 시험하지 못했다. `report` job은 이 시험 구성에서 파일·이벤트 설정이 없어 실패했고, 확인한 것은 `count` job의 요청 span 부모다.
- 6의 `prototype/tools/smoke.py`는 가드를 통과한 뒤 TypeScript 컴파일에서 `OperationBinding`이 prototype SDK에 없다는 기존 오류로 실패한다. 이번 변경과 무관해 고치지 않았다.
- 1은 Keycloak만 확인했다. Auth0·Entra ID는 계정이 없어 미실행이다. 발급 토큰의 `aud` 문자열 형태와 배열 형태 중 배열만 실제 provider로 봤다.
- main의 `spike-v6-transport`는 `cargo clippy --all-targets -D warnings`가 `src/server.rs:724`의 `items after a test module`로 실패한다. 다른 트랙의 변경이라 건드리지 않았다.

## WBS 8.5 보존 정리와 S3 owner quota (2026-10-18, Claude 구현 / 독립 검증 대기)

DB 테스트는 이 Run이 띄운 PostgreSQL 17 컨테이너(`aip-claude-wbs8-pg-1018`, `127.0.0.1:32801`, DB `aip_test_wbs8_1018`)에서만 실행했고 공유 5432에는 접속하지 않았다. 객체 저장소 live 테스트는 이 Run이 만든 임시 CA의 TLS MinIO 컨테이너(`aip-claude-wbs8-minio`, `127.0.0.1:32802`, 이미지 `aip-minio-fixture:local`, 서버 `RELEASE.2025-09-06T17-38-46Z`)에서 실행했다. AWS S3와 GCS 실계정은 미실행이다.

### 구현한 것

| 항목 | 결과 | 위치 |
|---|---|---|
| S3 owner별 합계 quota | 서명한 PUT·multipart part 크기를 `aip_infra_<digest>.aip_object_ledger`에 예약하고 owner 합계(기본 10GiB·1,000개, `objects.max_total_bytes`·`max_objects`)에 넣어 검사한다. 넘으면 URL 없이 `WIRE.QUOTA_EXCEEDED`. 같은 객체 재서명은 이전 예약을 대체하고 owner마다 advisory lock으로 동시 예약이 합계를 넘지 못하게 한다 | `crates/aip-execution/src/object_ledger.rs`, `spikes/spike-v6-transport/src/object_transport.rs` |
| 만료 업로드 정리 | 서명 PUT은 발급 65분 뒤, multipart는 part 서명이 24시간 없으면 서버가 `HEAD`로 저장소를 확인한다. 객체가 있으면 실제 크기로 확정, multipart는 없으면 저장소에서 abort하고 예약을 푼다. 서명 PUT은 첫 부재에서 풀지 않고 24시간 보류한 뒤 한 번 더 확인한다(2026-10-18 후속, [릴리스 게이트](RELEASE-GATE.md)). 서명 DELETE는 저장소가 객체 없음을 확인할 때까지 quota를 유지한다. 모든 인스턴스가 60초마다 돌되 행별 임대와 `version` 비교로 재서명과 겹쳐도 새 예약을 지우지 않는다 | 같은 파일의 `reconcile_loop`, `S3Store::head_object` |
| multipart 보호 | 장부에 없는 upload id의 part 서명과 complete는 `WIRE.NOT_FOUND`. 완료 객체의 id와 지어낸 upload id로 abort를 호출해도 그 객체의 quota가 풀리지 않는다 | `ObjectLedger::abort_multipart` |
| outbox 정리 | relay가 넘긴 행·dispatch 기록 보존 기간을 설정으로 열었다(`retention.outbox_dispatched_days`, 기본 30). handler가 없는 topic의 행은 `retention.outbox_unhandled_days`를 정할 때만 지운다(기본 보존). handler가 있는 topic의 미전달 행은 지우지 않는다 | `spikes/spike-v6-transport/src/durable.rs`(`collect_expired`) |
| events 정리 | 개수 상한(stream당 1,000)에 더해 `retention.events_days`를 정하면 그 기간이 지난 이벤트를 지우고 replay 하한을 올려 옛 cursor는 `WIRE.CURSOR_EXPIRED`를 받는다. 10분마다 최대 1,000 stream | `crates/aip-execution/src/events.rs`(`expire_older_than`) |

기존 문서의 "`aip_outbox`는 아직 자동 정리하지 않으며"는 틀렸다. O-20(`b68fa7b`)이 handler가 있을 때 dispatch 30일 뒤 정리를 이미 넣었다. 이번 회차는 그 기간을 설정으로 열고 handler 없는 topic과 events를 더했다. `MANAGED-EXECUTION.md`와 `INFRASTRUCTURE.md`를 고쳤다.

### 실행 결과

| 명령 | 결과 |
|---|---|
| `cargo test -p aip-execution --test object_ledger --locked` | 17 passed. 장부 예약·대체·동시 8개 예약·만료 해소·stale 해소 거부·multipart 부담·abort 경계·서명 DELETE |
| `cargo test -p aip-execution --test events --locked` | 13 passed(신규 3: 나이 기준 정리, stream 수 상한, 잘못된 설정) |
| `cargo test --locked --test object_ledger_s3` (spike-v6, 실제 HTTP 서버와 PostgreSQL, 저장소 연결 없음) | 4 passed, 3 ignored |
| 같은 명령에 `-- --ignored`, MinIO 환경 변수(`scripts/test-minio-object-storage.sh`가 설정) | 3 passed. 서명 PUT 업로드 뒤 만료 확정, 미사용 5MiB 예약 해소, 서명 DELETE 미사용·사용 두 경우, multipart 두 part 완료와 유휴 upload의 저장소 abort(abort 뒤 part PUT이 404) |
| `cargo test --locked --lib retention_tests` (spike-v6) | 4 passed. 기본은 handler 없는 topic 보존, 설정하면 나이로 삭제, id 순서로 2,500행 삭제하고 젊은 40행에서 멈춤, dispatch 기간 설정 |
| `cargo test -p aip-service --lib` 신규 2개 | S3 quota 기본값·거부값, retention 기본값·범위 |
| `cargo test --workspace --locked --no-fail-fast` | 427 passed, 0 failed, 5 ignored |
| `cargo test --locked --no-fail-fast` (spike-v6) | 226 passed, 0 failed, 3 ignored(live MinIO) |
| `cargo fmt --all --check`, spike-v6 `cargo fmt --check` | 통과 |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings`, 제품 crate와 spike-v6 `--all-targets` | 통과 |

변이 확인: 합계 검사 제거(3 실패), stale resolve의 version 비교 제거(1), abort가 아무 행이나 지움(1), events 정리가 replay 하한을 올리지 않음(1)이 모두 잡혔다. reconciler가 multipart abort를 하지 않는 변이는 live 테스트의 abort 뒤 PUT 확인(200 대 404)에서 잡혔다.

전체 실행 중 `aip-execution` `messages`의 `owner_checks_scope_claim_ack_and_dlq_and_expiry_cleanup_is_bounded`가 한 번 실패했다(TTL 25ms 시험으로 부하에 따라 흔들린다). 단독으로 3번 연속 통과했고 이번 변경은 messages 모듈에 닿지 않는다. 전체 재실행에서는 통과했다.

### 한계·미검증

- 장부가 생기기 전에 올린 객체는 quota에 세지 않는다. (2026-10-19 증분으로 바뀜) 소유자가 `POST /objects/confirm`을 호출하면 provider HEAD 크기로 소급 계상된다(Verify 39210323292·Object storage 39210323219). 호출하지 않은 객체의 자동 발견(bucket 목록 스캔·이벤트 대조)은 여전히 미구현이며 후속 8.5.1·8.5.2로 인계한다.
- (2026-10-18 후속으로 바뀜) 첫 확인 뒤에 끝나는 PUT은 이제 보류 후 확정된다. 두 번째 확인(약 25시간) 뒤에 끝나는 PUT은 여전히 세지 않는다. bucket 수명 주기 규칙이 필요하다. 상세는 [릴리스 게이트](RELEASE-GATE.md)의 "후속 증분".
- 업그레이드 중에 장부가 생기기 전에 시작한 multipart는 part 서명과 complete가 `WIRE.NOT_FOUND`다(재시작해야 한다).
- quota는 서명한 크기 기준이다. provider 비용이나 bucket 전체 용량을 대신하지 않는다.
- `aip_events.created_at`에 index가 없어 events 정리는 10분마다 테이블을 한 번 훑는다. 앱 migration 지문을 바꾸지 않으려는 선택이다. 매우 큰 events 테이블의 비용은 측정하지 않았다.
- GCS(`gcs_hmac`)는 같은 코드 경로이지만 실제 계정으로 실행하지 않았고 MinIO로만 검증했다. AWS S3의 `HEAD`·multipart abort 응답 차이는 미확인이다(MinIO는 존재하지 않는 upload의 abort에도 성공을 돌려줘 `NotFound` 경로는 live로 재현하지 못했다).
- 독립 검증 전이다. 완료가 아니라 "독립 검증 대기"다.

## WBS 8.11 8.9 후속: clippy 정리와 Kind::Io 시험 (2026-10-18, Claude 구현 / 독립 검증 대기)

DB 테스트는 이 Run이 띄운 PostgreSQL 17 컨테이너(`aip-claude-wbs8-pg-1018`, `127.0.0.1:32801`, DB `aip_test_wbs8_1018`)에서만 실행했고 공유 5432에는 접속하지 않았다.

- `commit_unknown_wire` 자체의 clippy는 이미 깨끗했다(`cargo clippy --all-targets --locked -- -D warnings`, spike-v6). 8.9 절에 남긴 사전 실패는 이 파일이 아니라 spike-v3·v4 테스트였고 셋을 고쳤다: v3 `general_update`의 불필요한 참조, v4 `cleanup_tests`의 `marker_processes`(Linux 테스트에서만 쓰여 비Linux에서 dead code라 `cfg(target_os = "linux")`), v4 `grant_tests`의 `len() != 0`. 동작은 바꾸지 않았다.
- `Kind::Io`: 호출자의 COMMIT 호출에서는 여전히 재현하지 못한다. 대신 접속 단계에서 가짜 서버가 시작 메시지를 읽고 RST로 닫게 해 실제 `Kind::Io` 오류(`error communicating with the server`)를 만들고 `connection_failed`가 이를 연결 오류로 판정하는지 고정했다(`io_error_during_startup_is_a_connection_failure`). Io 문구 판정을 지운 변이에서 이 테스트가 실패했다(12 passed, 1 failed).
- COMMIT 전 단절 시험은 8.9 회차에 이미 있다(`disconnect_while_beginning_the_transaction_is_a_safe_retry`, `..._after_update_...`, `..._after_idempotency_insert_...`, `commit_on_an_already_closed_connection_...`). 새로 더하지 않았다.

| 명령 | 결과 |
|---|---|
| `cargo clippy --all-targets --locked -- -D warnings` (spike-v3, v4, v6 각각) | 모두 통과 |
| `cargo test --locked --test commit_unknown_wire` (spike-v6) | 13 passed |
| `cargo test --locked --no-fail-fast` (spike-v3, spike-v4) | 모두 0 failed |

## Memoir Folder rename 실DB AIP 검증 (2026-10-17)

Infisical은 공식 공개판 `188cf58173019bf0fe59972ed1c7ef715bec979c`에서 프로젝트 권한이 normalized Membership→MembershipRole 관계를 거쳐 결정됨을 확인했다. 기본 Member의 Settings 편집도 `ee/` 코드에서만 확인할 수 있었고, [root LICENSE](https://github.com/Infisical/infisical/blob/188cf58173019bf0fe59972ed1c7ef715bec979c/LICENSE)는 MIT Expat, [`ee/LICENSE.md`](https://github.com/Infisical/infisical/blob/188cf58173019bf0fe59972ed1c7ef715bec979c/backend/src/ee/LICENSE.md)는 별도 상용 라이선스다. AIP 현재 정책 DSL로 그 관계 평가를 그대로 표현하려면 큰 계약 확장이 필요해 이 실험은 기각했다. Infisical source service/API/DB를 실행하거나 운영 서비스·secret에 접속한 검증은 없으며, Infisical 적용으로 계산하지 않는다.

대체 후보는 기록에 있던 `port-trials/memoir-notes`이며, 그 시험의 Note create/favorite/delete와 겹치지 않는 Folder rename 한 업무를 택했다. [기계적 source 비교·제외 범위](tests/fixtures/memoir-folder-rename/README.md)에 따라 Memoir committed source revision `26396070917ee065b988b0de296baf54f6266322`의 owner-scoped update와 UUID Folder model을 AIP resource/read/apply에 연결했다. 원본 저장소 worktree에는 36개 변경 파일이 있어 건드리거나 pull하지 않았다. 이 commit과 local `origin/main` ref는 같지만 원격은 fetch하지 않아 freshness는 확인되지 않았다.

전용 PostgreSQL DB마다 synthetic source-shaped `memoir_source`와 AIP-owned schema를 생성했다. 원본 서비스/API/DB는 시작하지 않았다. 따라서 결과는 **코드 기준 적용 / 실DB AIP 검증**이며 original E2E 동등성이나 Memoir 전체 포팅을 뜻하지 않는다.

| 실제 실행 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_TARGET_DIR=/Users/winterholic/development/projects/aip/aip/target AIP_TSC=/Users/winterholic/development/projects/aip/aip/spikes/spike-0-ts/node_modules/.bin/tsc node --test product/tests/memoir-folder-rename.test.mjs` | 4 passed, 0 failed, 0 skipped | CLI init/migrate/provision/bind/serve, 생성 TS binding strict typecheck, SDK offline build/install. Owner가 UUID folder를 읽고 `name/sortOrder`를 `Research/3`→`Engineering/7`로 바꿈. 무관 actor는 read 결과 0행, apply `WIRE.MISSING_TARGET`; 거부 후 새 연결 조회와 실제 AIP DB 값은 계속 `Engineering/7`. |

합성 원본형 row는 변경 전후 `Research|3`으로 유지됐고 AIP-owned row만 `Engineering|7`이 됐다. 테스트 종료 뒤 전용 DB와 runtime role을 제거했다. 초기에 발생한 parser/fixture SQL/ref ID 오류는 fixture를 바로잡은 후 회귀가 통과했으며 AIP core gap은 발견되지 않아 코어 제품 코드는 수정하지 않았다. 원본 `@RequiresPlan` entitlement, 이름 공백 validation 경계, parent folder 흐름, source API/DB와 전체 Note model은 범위 밖이다.

## WBS A: 외부 IdP·TLS reverse proxy 운영 smoke (2026-10-17)

`conformance/idp-proxy/idp/server.mjs`는 저장소 자체의 경량 RSA/JWKS/JWT fixture이며, 실제 Keycloak·Entra·Google 연동이나 RFC 9068 전체 conformance 적합성은 미검증이다. Caddy TLS reverse proxy(자체 서명 SAN 인증서) 환경에서 `aip service` 운영 인증 및 프록시 헤더 처리를 검증했다.

| 실행 시점 및 환경 | 실행 명령 | 결과 | 확인한 범위 및 상태 |
|---|---|---|---|
| 원 작업 (`cbaa401`, 2026-10-17 02:56) | `bash scripts/smoke-idp-proxy.sh` (`node conformance/idp-proxy/smoke-check.mjs`) | 17 passed, 0 failed (`[SUCCESS] All 17 checks passed!`) | Caddy HTTPS 앞단 통신, Origin/CORS 허용·차단, `x-request-id` 보존 및 자동 생성, 루프백 직접 연결(헤더 위조) 차단 정책, OIDC IdP HTTPS JWKS 검증 기반 정상 토큰 인증 및 읽기, 토큰 만료(`WIRE.TOKEN_EXPIRED`), 잘못된 aud/iss(`WIRE.UNAUTHENTICATED`), 위조 서명/지원하지 않는 typ 거부, 미등록 sub 거부 |
| 독립 교차 검증 (`cbaa401`, 2026-10-17 03:07, 별도 DB `aip_flash_verify_20261017`) 및 후속 진단 | `AIP_TEST_DATABASE='...' bash scripts/smoke-idp-proxy.sh` | exit 1 (failed) | IdP health/discovery/JWKS 3 assertion 통과 후 `aip service` 시작 단계에서 `SERVER_CONFIG`로 중단됨. 후속 진단에서 실패 stage는 `worker_runtime_probe`로 확인됨: macOS worker 격리(`isolation=auto`) 아래 `node --version` 프로브가 exit status 71 (`stderr_class=permission_or_sandbox_denial`)로 종료. 이는 nested Mac Run 실행 환경의 제약이며, 어떤 syscall/프로파일 규칙이 거부됐는지는 특정되지 않음(정확한 원인은 불명). 이를 production 부재로 일반화하지 않으며, 프록시·CORS·request ID·JWT 검증 등 남은 14개 assertion은 여전히 미실행 상태임. |

- 사설/자체 서명 CA 환경 지원을 위해 `crates/aip-auth`가 `SSL_CERT_FILE` 우선, `AIP_AUTH_CA_PEM` fallback으로 환경 변수를 인식해 webpki 루트 인증서 풀에 추가하도록 지원한다. 파일 읽기/파싱 실패 시 `.ok()`로 조용히 무시하고 기본 루트로 계속하는 기존 동작을 유지한다. 두 회귀 테스트는 실제 local TLS/JWKS 요청에서 `new_explained`와 공유하는 lookup helper의 CA 로딩 경로, CA 부재·잘못된 CA 경로·hostname mismatch 거부를 검증했다. 공개 `new_explained` wrapper의 환경 조회 위임은 코드 경로로 확인했다.

## WBS B: 실제 Kafka·RabbitMQ 및 MinIO object storage smoke (2026-10-17)

실제 Kafka(v4.1.0 KRaft), RabbitMQ(v4.2), MinIO(S3 호환 HTTPS) 컨테이너를 구동하고 broker 발행·소비·재시도·redrive 및 object storage 업로드·다운로드·권한 경계를 실제 통신으로 검증했다. 원 작업과 독립 교차 검증 모두 4개 테스트 통과를 확인했다.

| 실제 실행 명령 (`scripts/smoke-brokers-storage.sh` 원문 일치) | 결과 | 확인한 테스트 및 범위 |
|---|---|---|
| `cargo test --locked -p aip-execution --features external-brokers --test brokers_live -- --ignored --nocapture` | 명령군 2 passed, 0 failed | `real_kafka_confirm_and_import_round_trip` (Kafka 발행·소비 왕복 및 dead-letter redrive), `real_rabbitmq_confirm_and_import_round_trip` (RabbitMQ 발행 확인 및 DLX quarantine 재시도) |
| `cargo test --locked -p aip-service --features external-brokers broker_worker_recovers -- --ignored --nocapture` | 명령군 1 passed, 0 failed (31 filtered out) | `brokers::tests::broker_worker_recovers_from_database_disconnect_without_losing_delivery_budget` (DB 연결 단절 후 broker 복구 복원력, audit_cli/jobs_cli 등 0개 타깃 제외) |
| `cargo test -p aip-execution --test cloud_storage minio_https_actual_object_and_multipart_contract --locked -- --ignored --nocapture` | 명령군 1 passed, 0 failed (3 filtered out) | `minio_https_actual_object_and_multipart_contract` (MinIO TLS S3 호환 다중 파트 업로드·다운로드 및 읽기 권한 분리) |
| **전체 스크립트 실행**: `bash scripts/smoke-brokers-storage.sh` | **총계 4 passed, 0 failed** | 상기 세 명령군을 순차 실행하여 4개 live 통합 검사 모두 통과 (필터된 0개 타깃은 집계에서 제외) |

- `crates/aip-execution/tests/brokers_live.rs` 및 `crates/aip-service/src/brokers.rs` 내 하드코딩되어 있던 `user=runner` 연결 문자열을 `AIP_TEST_DATABASE` 및 `AIP_TEST_DATABASE_URL` 환경 변수를 우선 참조하도록 수정했다.
- MinIO 도커 환경은 `conformance/brokers-storage/minio/Dockerfile`에서 고정 릴리스 태그(`RELEASE.2025-09-06T17-38-46Z`) 및 SAN 인증서 진입점을 구성했다. 독립 검증에서는 기존 로컬 이미지(`aip-minio-fixture:local`)가 재사용되어 source build/download 경로는 미실행되었다. '공식 이미지 배포 중단'은 확인된 근거가 없으므로 단정에서 제외한다.

# 제품 통합 검증 (2026-10-06)

## MySQL 8.4 읽기·쓰기·작업 저장소 실행 검증 (2026-10-18, Claude 구현 / 독립 검증 대기)

범위는 [MySQL adapter 계약](../plan-docs/porting/MYSQL-ADAPTER-CONTRACT.md)의 "읽기 경로", "쓰기 경로", "작업 저장소" 절이다. 실행 환경은 macOS arm64, Docker의 `mysql:8.4`(8.4.11)와 `mariadb:11.4`(11.4.13)를 `scripts/mysql-live-fixture.sh`로 loopback·tmpfs에 띄운 격리 컨테이너, 격리 PostgreSQL 17이다. 업무 DB·`.env`는 쓰지 않았다. 모든 라이브 시험은 `#[ignore]`이고 `-- --ignored` 또는 `--include-ignored`로 실행하며, 대상 URL이 없으면 통과가 아니라 실패한다.

| 명령(요약) | 결과 |
|---|---|
| `cargo test -p aip-migrate --features mysql -- --include-ignored mysql --skip every_semantic_ddl` (MySQL) | 20 passed(드라이버 라이브 13 포함) |
| 같은 명령 (MariaDB URL) | 20 passed |
| `cargo test -p aip-migrate --features mysql --test mysql_fence_verification -- --include-ignored` (MySQL+MariaDB URL 동시) | 8 passed. 두 엔진 모두 |
| `cargo test -p aip-migrate --features mysql --test mysql_drift_matrix -- --ignored` | 1 passed: 35개 DDL 변경 중 의미 있는 32개가 지문을 바꾸고 트리거·view·comment 3개가 바꾸지 않음(알려진 한계로 고정) |
| `cargo test -p aip-migrate --features mysql --test mysql_jobs -- --ignored` | 3 passed: job 상태 기계, 40 run×8 worker 동시 claim, outbox 릴레이 |
| `cargo test -p aip-migrate --features mysql --test mysql_capability_gate -- --ignored` | 2 passed: MySQL 8.4 통과, MariaDB 11.4 거부(LATERAL, FOR SHARE OF) |
| `cargo test -p aip-service --features mysql --test mysql_read_http --test mysql_write_http -- --ignored --test-threads 1` | 읽기 1 + 쓰기 5 passed |
| `cargo test -p aip-cli --features mysql --test mysql_adoption_flow -- --ignored` | 1 passed: 실제 `aip` 바이너리로 plan/apply/bind/serve/JWT 읽기·쓰기/drift |
| `cargo test -p aip-migrate -p aip-service` (기본 feature) | 121 passed, 1 ignored |
| `cd spikes/spike-v2-read && cargo test` (PostgreSQL) | 109 passed. 방언 도입 뒤에도 PostgreSQL SQL이 같다 |
| `cd spikes/spike-v6-transport && cargo test --no-fail-fast` | 65 target 통과, 5 target 실패(`v12_typed`·`v13_idwire`·`v14_apply`·`v15_values`·`v16_bundle`). 다섯 모두 `tsc` 부재(`Os { code: 2, NotFound }`)로 기존 사전 실패와 같다 |
| `cargo clippy -p aip-migrate -p aip-service -p aip-cli --features mysql,aip-cli/mysql --all-targets -- -D warnings` | 경고 0 |

정정(2026-10-18 후속): 위 표의 `mysql_adoption_flow`·`mysql_read_http`·`mysql_write_http`·`mysql_jobs` 결과는 서버 능력 게이트(`check_server_profile`)를 넣기 전 실행이다. 게이트 뒤에는 `mysql_adoption_flow`가 실패하는 상태로 푸시됐다. 시험 런타임 계정에 `CREATE TEMPORARY TABLES`가 없어 probe가 서버를 "InnoDB·SKIP LOCKED·CHECK 없음"으로 오판했고, `mysql_jobs`는 두 시험이 같은 schema를 병렬로 지워 서로 깨뜨렸다. 고친 뒤 같은 명령을 다시 실행했다: `mysql_adoption_flow` 1 passed, `mysql_read_http` 1 + `mysql_write_http` 5 passed, `mysql_capability_gate` 3 passed(권한 부족 계정이 서버 결함이 아니라 권한 부족으로 거부되는 시험 추가), `mysql_jobs` 3 passed(직렬화), `mysql_fence_verification` 8 passed, `mysql_drift_matrix` 1 passed. 런타임 계정은 업무 schema에 `CREATE TEMPORARY TABLES`가 필요하다(세션 안에서만 보이는 probe 표 하나를 만든다).

2026-10-18 후속 실행(같은 컨테이너, 같은 명령 형식): `AIP_TEST_JOBS_URL`을 MariaDB로 두고 `cargo test -p aip-migrate --features mysql --test mysql_jobs -- --include-ignored`를 실행하면 3 passed(상태 기계·8 worker 동시 claim·outbox 릴레이)이고, MySQL 8.4에서도 3 passed다. MariaDB에서 claim의 `SKIP LOCKED`를 빼면 `mysql_concurrent_claims_never_hand_a_run_to_two_workers`가 실패한다. `mysql_capability_gate`는 4 passed(job 저장소 프로파일은 MariaDB를 통과시키고 서비스 프로파일은 계속 거부). `mysql_write_http`는 6 passed(`where` target, `sameScope`, 단일 `update` 효과 추가). 이 항목의 "MariaDB는 job 저장소를 실행하지 않았다"는 서술은 이로써 낡았다.

요청당 catalog 비용(측정, `product/tests/fixtures/facereview-mysql/reference/big.py`): 펜스는 캐시 없이 요청마다 바인딩된 모든 표의 catalog를 읽는다. 표 하나에 약 8.8ms(중앙값), 10개 13.5ms, 30개 22.5ms, 60개 35.7ms, 120개 62.6ms로 표당 약 0.45ms씩 선형으로 늘었다(p95는 중앙값보다 3~4ms 위). FaceReview 9개 표는 읽기 요청당 약 13.7ms다. 조건은 debug 빌드, 같은 호스트의 loopback MySQL 8.4, 표당 열 10개, 요청 100회 단일 실행이다. 운영 환경의 지연과 같다고 주장하지 않으며, 표 수가 수십 개를 넘는 배포에서는 캐시 없는 fail-closed 설계의 비용이 눈에 띈다는 것이 확인된 내용이다.

환경 한계: `aip-cli`의 `least_privilege`는 비밀번호 인증 PostgreSQL에서는 역할 접속이 실패하고, trust 인증 PostgreSQL 컨테이너에서는 통과한다(이번 변경과 무관). 시험 DB 비밀번호와 계정은 모두 이 시험을 위해 만든 일회용이다.

변이 확인(보호 장치를 하나씩 끄고 시험이 실패하는지 확인): 용량 잠금, 키 잠금, 매칭 행 수 확인, SERIALIZABLE(`mysql_write_http`), 후보 행 잠금, lease epoch 검사, epoch 증가(`mysql_jobs`)는 각각 시험을 실패시켰다. 쓰기 판정의 `FOR UPDATE`만은 끄면 통과하며, 뒤의 `UPDATE`가 같은 행 잠금과 `from` 재확인을 해서 결과가 같기 때문이다.

미실행·미검증: MariaDB의 읽기·쓰기 경로(능력 부족으로 거부), HTTP `/jobs/*`·job 실행, `compose`, 확장 쓰기, OIDC 외부 IdP로 UUID actor를 묶는 CLI 경로의 IdP 연동(UUID actor 바인딩 자체는 FaceReview 시험이 실행), 다른 OS. 이 문장은 이후 실행으로 일부 갱신됐다(아래 후속 단락).

## MySQL catalog 변환 계층과 fail-closed 경계 (2026-10-17)

AIP external introspection은 현재 PostgreSQL 전용이다. MySQL URL 거부 guard는 유지하면서, `aip-migrate::mysql_catalog::normalize_table`에 driver 비의존 INFORMATION_SCHEMA 행→중립 manifest 변환을 추가했다. 원본 `facereview-inventory.json`의 11개 SQLAlchemy 모델과 PostgreSQL catalog/외부 채택 경로를 대조해 컬럼 타입·PK/인덱스 순서의 작은 범위로 제한했다. 아래 catalog 행들은 synthetic 회귀 fixture이며 실제 DB 관측이 아니다.

| 실행 | 결과 | 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/aip-target cargo test -p aip-migrate --test mysql_catalog --test mysql_adapter --locked --offline` | 9 passed, 0 failed | synthetic PK/index/FK 순서·unsigned·enum·tinyint(1) alias·nullable/default/generated·NULL default 모호성·varchar 길이 진단·잘못된 metadata 거부, 그리고 기존 URL fail-closed/credential 숨김 |
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/aip-target cargo test -p aip-migrate --test mysql_catalog --test mysql_adapter --test migrate --locked --offline` | 38 passed, 0 failed | catalog 8, 기존 MySQL URL guard 1, migrate regression 29 |
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/aip-target cargo test -p aip-migrate --lib --locked --offline` | 1 passed, 0 failed | migration library 회귀 |
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/aip-target cargo clippy -p aip-migrate --all-targets --locked --offline -- -D warnings` | exit 0 | 변경 crate 전체 target lint |
| `cargo fmt --all --check` | exit 0 | workspace Rust formatting |
| `python3 tools/doccheck.py` | 14 aip blocks checked, 0 findings | 제품 문서 선언 검사 |
| `python3 tools/check_plan_docs.py` | 172 markdown files checked, 0 errors | plan-docs 검사 및 negative controls |
| `git diff --check` | exit 0 | 공백 오류 없음 |
| `CARGO_NET_OFFLINE=true cargo build -p aip-cli --locked --offline && node --test product/tests/uuid-external.test.mjs` | build exit 0; PostgreSQL UUID owned/external 통합 1 passed, 0 failed | 기존 PG introspect/adopt, runtime DML role, TS/Python SDK, 두 wire, drift 거부. 새 MySQL converter를 호출하지 않음 |

`mysql_catalog` 테스트 입력은 테스트 코드에서 만든 synthetic catalog rows다. DB에서 관측한 값으로 표시하거나 취급하지 않는다. MySQL 8.4는 `INFORMATION_SCHEMA.COLUMNS.COLUMN_DEFAULT = NULL`을 명시적 `DEFAULT NULL`과 DEFAULT 절 부재 양쪽에 반환하므로, manifest는 이를 `NullOrAbsent`로 남기고 `SHOW CREATE TABLE` 등 추가 판별을 요구하는 diagnostic을 붙인다 ([공식 MySQL 8.4 문서](https://dev.mysql.com/doc/refman/8.4/en/information-schema-columns-table.html)). converter는 typed columns/index rows와 FK 입력을 받으며, FK manifest는 composite 순서·source/target·constraint identity·actions를 보존한다. 잘못된 순서/누락/미지원 action은 거부하고 `CHAR`/`VARCHAR` 길이 축약은 diagnostic으로 남긴다. 원본 11개 RDB 모델 선언은 [FK 기준 manifest](../plan-docs/porting/facereview-fk-manifest.json)에 정리했다. 기존 댓글 PostgreSQL fixture는 Comment의 두 참조만 보여 주며 11개 전체의 catalog 관측치나 동등성 근거가 아니다. 진단 항목은 소비자가 검토해야 하며 unsupported 타입은 manifest에 명시한다.

이번 후속 변경은 중복/누락 ordinal·없는 index column·혼합 `NON_UNIQUE`·`VARCHAR` 길이 진단 음성 회귀를 추가했다. 수정 전 duplicate ordinal, missing index column, mixed uniqueness, missing length diagnostic은 각각 원래 오류 또는 손실을 재현했고 수정 뒤 통과했다. composite FK 순서/분리, schema/table/column target, action 보존과 잘못된 순서·누락·알 수 없는/`SET DEFAULT` action 거부도 synthetic 입력으로 통과했다.

driver 추가, MySQL 실 DB 연결·catalog/read/write, MariaDB 실행은 미실행이다. adapter는 미지원 상태다. 이전 `cargo info mysql_async` 단계는 crate 다운로드 뒤 nested worker sandbox에서 Cargo cache open이 `Operation not permitted`로 막혔다. 이는 해당 실행 환경의 cache 접근만 말하며 production의 MySQL 가용성 원인으로 일반화하지 않는다. cache 경로 변경·직접 다운로드·브라우저/권한 우회는 하지 않았다. 다음 단계는 허용 의존성 공급이 가능한 실행 환경에서 driver와 read-only query를 연결하고 같은 manifest 회귀를 실제 catalog row로 대조하는 것이다.

이번 catalog/FK 구현 source revision: `28bb463f4bb73f461ff813ec8da3f57ad726e165` (코드·테스트·원본 FK manifest 커밋).

운영 인증·일반 마이그레이션·root 제품 통합을 사용자 요청 범위로 구현했다. AIP의 caller read/direct apply와 서버의 최종 권한, 같은 정의·같은 SDK·Node/Python 확장을 유지한다. 앱별 endpoint나 별도 실행 엔진을 추가하지 않았다.

## 페이스리뷰 원본 Socket.IO 부분 기준선 (2026-10-17)

고정 원본 backend `7948b7e721837fdb73c427cb54e12cad4ec2d5af`를 기존 격리 seed에서 실행해 허용된 `http://localhost:5173` Origin으로 Engine.IO polling handshake를 열었다. 실제 빈 `watch_frame` 입력은 `Missing required fields`로 거부됐고 client transport 종료를 확인했다. 결과는 [기계 판독 보고서](../plan-docs/porting/facereview-socket-baseline.json), 재현 gate는 `tools/facereview_socket_baseline.py`와 `tools/test_facereview_socket.py`에 있다.

| 실행 | 결과 | 범위 |
|---|---|---|
| `/tmp/aip-facereview-baseline-venv/bin/python -m unittest discover -s tools -p 'test_facereview_socket.py'` | 4 passed, 0 failed | 고정 source/Origin/transport/응답·부분 범위, scope 과장 및 민감 필드 거부 |
| `/tmp/aip-facereview-baseline-venv/bin/python tools/facereview_socket_baseline.py --source /tmp/aip-face-comment-ui/backend --inventory plan-docs/porting/facereview-inventory.json --output /tmp/aip-face-comment-socket-r2.json` | `ok=true`, connect pass, transport closed, `Missing required fields` | 실제 원본 polling handshake와 빈 입력 거부 |

격리 재실행 서버 로그는 client transport 종료 뒤 `handle_disconnect()`의 필수 `message` 인자가 없어 `TypeError`가 난 것을 보였다. 원본 callback은 실패했으며 이 원본 결함은 AIP 갭으로 분류하지 않았다. WebSocket transport는 Python `websocket-client` 부재로 미실행이고, 프레임·ML·Mongo timeline·worker/provider·AIP 대조는 미실행이다. 이 단위는 AIP 본체 변경이나 제품 갭 발견으로 계산하지 않는다. 상세 환경은 [페이스리뷰 포팅 추적](../plan-docs/porting/README.md)을 따른다.

## 내부 metrics·OTLP exporter·durable job trace 연결 검증 (2026-10-17)

WBS 항목인 "내부 metrics·OTLP exporter·durable job trace 연결"의 3개 범위를 구현하고 검증했다.

| 실제 실행 | 결과 | 확인한 범위 |
|---|---|---|
| `cargo test --test otlp --manifest-path spikes/spike-v6-transport/Cargo.toml --target-dir target` | 2 passed, 0 failed | mock in-process OTLP/HTTP 수신기(/v1/traces, /v1/metrics)를 통한 trace/span 수신, HTTP 요청 span과 durable job run span의 동일 trace_id 연계, duration histogram 수신, 민감값(인증 토큰·payload) 배제 |
| `cargo test -p aip-execution` | 80 passed, 0 failed | `aip_jobs.trace_id` 스키마 바인딩·보존(18 tests), files(5), integrations(10), messages(15), objects(11), rag(16), rate_limit(4), trace_context(4) |
| `cargo test -p aip-service` | 33 passed, 0 failed | 제품 설정의 `otlp` 파싱 및 validation, unit tests(31), audit_cli(1), jobs_cli(1) |
| `cargo fmt -- --check && cargo fmt --manifest-path spikes/spike-v6-transport/Cargo.toml -- --check` | exit 0 | workspace 및 transport spike 코드 포맷 검사 |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings` | exit 0 | workspace 라이브러리/바이너리 clippy 검사 |
| `cargo clippy -p aip-auth -p aip-migrate -p aip-service --all-targets --locked -- -D warnings` | exit 0 | 제품 서비스 크레이트 전체 타깃 clippy 검사 |
| `cargo clippy --manifest-path spikes/spike-v6-transport/Cargo.toml --all-targets --locked -- -D warnings` | exit 0 | transport spike 전체 타깃 clippy 검사 |
| `cargo deny --manifest-path Cargo.toml check && cargo deny --manifest-path spikes/spike-v6-transport/Cargo.toml check` | exit 0 | 라이선스, 금지 crate, advisories, sources 보안 검증 |

미검증:
- 실제 remote OTLP Collector 실연결은 미실행. 별도 격리 검증에서는 로컬 Collector 0.120.1가 실제 제품 HTTP span과 `export_job_span` wire를 수신·파싱해 HTTP 200→`Ok`, 405→`Error`, job `SUCCEEDED`→`Ok`, `FAILED`/`CANCELLED`→`Error`, `RETRY`→`Unset`을 확인했다. Job 검증은 synthetic Lease와 제품 emitter를 사용했으며 PostgreSQL enqueue/claim 또는 worker lifecycle은 포함하지 않는다.

## 기존 durable job schema의 trace_id upgrade 후속 (2026-10-17)

출시 차단 결함: `4e8e2126a91bd15e21afd870fe5373c0ccba9adb` 이전에 생성한 `aip_jobs`에는 `trace_id`가 없다. 기존 metadata의 DDL digest가 현행 생성 DDL과 다를 때 planner가 `verified_marker()`에서 거부해 upgrade plan을 만들지 못했고, 그 상태의 Store enqueue/claim은 PostgreSQL `42703`을 반환했다. 이 원인 경로는 기존 재현의 실행 결과에 코드 경로 검토를 결합한 것이다.

수정은 이전 job DDL digest를 정확히 재구성해 해당 marker만 별도 인식한다. facts/generator version·journal·실제 catalog digest 검사는 유지하고, 실제 `trace_id`가 이미 있거나 다른 drift·미지 digest가 있으면 계속 거부한다. 허용된 schema는 plan에 nullable 컬럼 추가를 기록하고 기존 advisory fence·transaction·rollback·target verification·metadata/journal commit을 거친다. 범용 migration version 체계는 추가하지 않았다.

| 실행 | 결과 | 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=<Run temp> cargo build --locked --offline -p aip-cli` | exit 0 | 제품 `aip service` CLI 빌드 |
| `AIP_TRACE_UPGRADE_DATABASE_URL=<Run-owned PostgreSQL> AIP_SERVICE_CLI=<built aip> CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=<Run temp> cargo test --locked --offline -p aip-migrate --lib trace_id_upgrade_tests::explicit_parent_job_schema_upgrade_adds_trace_id_and_rejects_drift -- --ignored --exact --nocapture` | 1 passed, 0 failed | test가 실제 `aip service init` 및 `aip service migrate` plan/apply/repeated apply CLI를 호출. 기존 NULL job claim·retry, 신규 enqueue/claim·idempotency replay·trace retry 보존, actor/principal/idempotency rows와 facts/wire metadata 보존, unknown digest·추가 drift·이전 digest와 실제 컬럼 불일치 거부 |

위 표와 이 문단은 구현 worker의 최초 자체 검증 기록이다. 테스트는 별도 Run 소유 PostgreSQL 컨테이너의 임의 loopback 포트와 전용 DB를 사용했다. 부모 job 구조는 독립 재현에 기록된 이전 DDL(현재 생성물에서 `trace_id`만 제거)과 그 정확한 DDL/catalog marker로 구성했다. SQL을 직접 실행해 컬럼을 성공시킨 결과는 PASS로 세지 않았고, SQL 조작은 과거 schema 재현 fixture 준비에만 썼다. 최초 자체 회귀에서는 적용 후 실패를 강제한 rollback 및 제한된 DB role 권한 음성 경로를 실행하지 않았다. 그 실행 시점에는 apply의 advisory lock·단일 transaction·목표 검증·metadata/journal commit 경로는 코드 경로 확인에 그쳤다.

이후 고정 source SHA `9bc78de3654c163e29f3cb369ec21ff496499e85`를 별도 worktree와 별도 Run 소유 PostgreSQL에서 독립 검증했다. **이 SHA의 DB upgrade 범위 `verification_outcome=passed`**다. CLI `init` → `migrate plan` → `apply` → repeated `apply`를 통해 기존 NULL trace job과 신규 enqueue/claim/retry/replay, actor/principal/idempotency/facts/wire 보존, drift·미지 digest 거부를 확인했다. DDL 실패 주입은 제품 apply를 `MIGRATION_FAILED`로 끝냈고 컬럼 미추가·metadata/catalog 불변·journal 1→1 rollback을 확인했다. 제한 role은 `DB_APPLY`의 `journal 제약 정리 실패` 단계에서 거부됐으므로 ALTER 권한 단계까지 도달한 검사로 확대하지 않는다.

원시 독립 로그에는 최종 PASS 전의 두 번의 관측 helper 실패도 남아 있다. 첫 시도는 제한 role 정리 중 종속 객체 때문에 helper가 종료됐고, 다음 시도는 rollback 구조 비교식의 자기 비교 오류로 실패했다. helper를 바로잡고 원복한 최종 반복은 exact regression `1 passed; 0 failed`였고, 위의 rollback 및 제한 role 관측을 출력했다. 이전 코드의 원래 결함은 commit `4e8e212` 기준 planner `WIRE.SCHEMA_MISMATCH`와 Store `42703`이며, 수정 후 최종 독립 PASS와 시점을 구분한다.

OTLP 회귀 4개는 이 candidate에서 실행했다. Collector 실연결은 새로 실행하지 않았다. `661024d6d23629b2eb276105eaba22144f0ff401`와 비교한 `durable.rs`, `lib.rs`, `otlp.rs`, `trace.rs`, `tests/otlp.rs` 다섯 파일이 모두 byte-identical이므로 이전 Collector 실행 근거만 재사용한다. 실제 worker E2E는 미실행이고, 제한된 worker runtime probe의 기존 sandbox denial을 넘어 확인하지 않았다. plan-docs의 `D-development-experience.md` 관련 기존 broken links 12개는 별도 상태로 유지하며 이번 문서/DB 검사 통과로 바꾸지 않는다. 이 판정은 DB upgrade 한정이다. 전체 WBS와 AIP 출시 완료는 미완이고, production 배포도 하지 않았다.

## 페이스리뷰 댓글 적용 시험 (2026-10-15)

사용자가 AIP 미완료 작업을 이어가며 페이스리뷰 등 개인 프로젝트·공개 저장소로 시험하는 방식을 허용했다. [페이스리뷰 댓글 slice](tests/fixtures/facereview-comments/README.md)를 먼저 선택했다. 원본 `new-facereview` HEAD `7948b7e721837fdb73c427cb54e12cad4ec2d5af`의 모델·서비스·입력 schema를 읽고 PostgreSQL 복사 구조를 만들었다. 원본 MySQL·MongoDB와 운영 데이터를 수정하지 않았다.

| 실제 실행 | 결과 | 범위 |
|---|---|---|
| `node --test product/tests/facereview-comments.test.mjs product/tests/general-update.test.mjs product/tests/uuid-external.test.mjs` | 22 passed, 0 failed, 0 skipped | 페이스리뷰 19개 하위 시나리오와 상위 test, 기존 Memoir 일반 수정, UUID owned/external·두 wire |
| `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --target-dir target` | 33 passed, 0 failed, 0 ignored | 쓰기 엔진 전체 unit/integration regression. 새 scalar literal test 2개 포함 |
| `cargo clippy --manifest-path spikes/spike-v3-write/Cargo.toml --lib --locked --target-dir target -- -D warnings` | exit 0 | 변경 엔진 library lint. 기존 needless borrow 한 곳도 정리 |
| `cargo build -p aip-cli --locked` | exit 0 | 위 HTTP/SDK 시험 전에 최신 CLI 재빌드 |
| `cargo fmt --manifest-path spikes/spike-v3-write/Cargo.toml --check`, `python3 tools/doccheck.py` | exit 0, 14 aip blocks checked, 0 findings | 변경 crate 형식과 문서의 제품 선언 블록 |

외부 PG의 `Text(1..500)` 생성에 빈 content가 저장되는 실패를 실제 HTTP로 재현했다. 신규 unit test에서도 범위 검증 누락과 Time/Url 생성 미지원으로 2개가 실패했다. 생성·patch·expect를 같은 scalar/range parser에 연결한 뒤 위 결과를 얻었다. 영상이 나중에 삭제되면 일반 댓글 조회에서 남는 앱 정책 누락도 음성 시나리오로 재현하고 선언에 영상 가시성 조건을 추가했다.

환경은 macOS arm64·로컬 PostgreSQL 17.11·메모리 생성 JWT key·설치 TS SDK다. Python은 생성 helper와 로컬 SDK package 경로를 사용했다. 테스트는 새 DB와 자체 runtime 역할만 만들고 종료 시 정리한다. 원본 MySQL에 직접 붙이기, 원본 frontend/API 호환, 관리자·좋아요/북마크·감정 분석·외부 provider·worker 격리·운영 배포는 검증하지 않았다. `tools/verify.sh`에 새 시험을 연결했지만 이번 회차에 전체 core script를 실행한 것은 아니다. 자세한 적용 차이와 다음 우선순위는 slice README를 따른다.

## Linux 격리 worker/job smoke (2026-10-14)

Ttakkari 안의 macOS에서는 `sandbox-exec`가 `Operation not permitted`로 차단되어 worker가 시작되지 않았다. 이를 대신해 `.github/workflows/worker-jobs-linux.yml`을 추가하고 GitHub Actions Ubuntu 24.04에서 기본 격리(`auto` → Bubblewrap)가 실제 적용되는 경로를 실행했다. 새 workflow는 별도 PostgreSQL 17 service를 사용한다.

| 실행 | 결과 | 확인한 범위 |
|---|---|---|
| GitHub Actions [Linux worker jobs #38511151991](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38511151991), commit `1d5a666` | success | Ubuntu 24.04, PostgreSQL 17, Bubblewrap 경계 14 passed; 격리 경계의 파일·쓰기·네트워크 차단 테스트 포함 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test job_retry --test triggers --test outbox` (workflow step) | 6 passed, 0 failed | Node/Python의 job 성공·`retryLater(afterMs)` 후 재실행, cron schedule, HMAC 서명 webhook, `notify` outbox 행에서 durable job 실행 |
| `node --test product/tests/service-smoke.test.mjs` (workflow step) | 1 passed, 0 failed | 설치 SDK와 제품 CLI의 실제 PostgreSQL smoke; Node/Python worker의 job 성공을 두 Id wire에서 확인 |

위 통합 테스트들은 각각 Node와 Python worker를 직접 순회한다. webhook 검사는 서명된 inbound webhook의 job 수락·실행·중복 전달, cron은 서비스 주체 job 실행, outbox는 commit된 notify 전이 뒤 정확히 하나의 성공 job을 확인했다. GitHub Actions의 후속 push run #38511151991은 전체 workflow success다. 앞서 수동 dispatch run #38510801999도 세 검증 step을 모두 통과했지만, 동시 main push가 마지막 정리 단계 중 들어와 workflow 상태는 cancelled로 기록됐다. 결과 기준은 전체가 success인 #38511151991이다.

로컬 컨테이너 대체 경로는 사용하지 못했다: Docker·Colima·Podman 실행 파일이 없었고, 설치된 OrbStack CLI도 Ttakkari sandbox에서 `~/.orbstack` 초기화가 `Operation not permitted`로 거부됐다.

macOS의 Seatbelt 정책 자체는 Linux 결과로 검증되지 않았다. 일반 macOS Terminal에서 로컬 PostgreSQL과 SDK test dependency가 준비된 상태로 다음 명령을 한 번 실행하면 실제 Node/Python worker와 job smoke를 확인할 수 있다. 이 스크립트는 임시 HOME/npm cache와 새 임시 schema를 사용한다.

```sh
cd /Users/winterholic/development/projects/aip/aip
npm ci --prefix spikes/spike-0-ts --ignore-scripts --no-audit --no-fund # node_modules/.bin/tsc가 없을 때만
bash scripts/verify-macos-worker-smoke.sh
```

기대 출력은 `✔ the installed SDK completes authenticated reads, cache, writes and operations for both Id wires`, `ℹ pass 1`, `ℹ fail 0`이다. Ttakkari 안에서는 이 macOS 검사를 실행하지 못했으며, Terminal에서 위 스크립트를 실행하기 전까지 macOS sandbox 정책은 미검증이다.

## 계약 응답·내보내기 보완 (2026-10-14)

CD-13·CD-2·CD-7을 반영한 회차다. `/read`·`/apply`·`/status`의 계약 헤더 누락과 지문 불일치를 구분하고, JSON 실패 본문과 SDK에 요청 ID·재시도 지침을 연결했다. 미확정 쓰기 오류에도 마지막 서버 지침이 남으며 기존 멱등 키 복구를 유지한다. `aip contract`는 compact가 기본이고 `--pretty`가 같은 JSON 의미를 보존한다. 최상위 오류 표의 참조를 확장된 계약과 비교해 코드·reason 손실이 없는지 확인했다.

| 실행 | 결과 | 범위 |
|---|---|---|
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test …` | 선택한 14개 대상의 26개 테스트 통과 | `error_body`, `v13_headers`, `wire_codes`, `v13_idwire`, `bundle_http`, `v12_typed`, `v14_apply`, `v8_auth`, `response_connection`, `v10_session`, `v9_lifetime`, `configured_server`, `v6_e2e`, `absence_serializable_http`. 실제 PG·HTTP·SDK 경계 포함 |
| `node --test` (verify.sh의 SDK·transport 단위 테스트 16개 파일) | 107 passed, 0 failed | 캐시·세션·쓰기 복구·계약·관리 API·실패 metadata |
| `python3 -m unittest product.tests.python_sdk_test` | 21 tests, OK | Python 호출 SDK. 기존 네트워크 경계 테스트에서 ResourceWarning은 남음 |
| `npm_config_cache=/private/tmp/aip-E-npm-cache node --test product/tests/sdk-package.test.mjs` | 5 passed, 0 failed | offline pack/install·실제 소비자·strict 타입. 임시 npm cache를 사용해 기본 cache의 실행 환경 쓰기 제한을 피함 |
| `cargo test -p aip-cli --locked --test contract_golden` | 4 passed, 0 failed | compact·pretty의 의미 일치, 코드·reason 보존, 생성 TS 골든과 운영 정보 분리 |
| `cargo test -p aip-ir --test codes --locked` | 8 passed, 0 failed | 오류 레지스트리와 방출 위치 pin |
| `cargo clippy --workspace --lib --bins --locked -- -D warnings`, transport의 `clippy --all-targets` | exit 0 | 제품 실행 코드와 transport 테스트 lint. 새 CLI 계약 테스트도 별도로 lint |

전체 통과를 주장하지 않는다. `bash tools/verify.sh`는 `service_flow`의 macOS worker 기동에서 실패했다. `/usr/bin/sandbox-exec -p '(version 1)(allow default)' /usr/bin/true`도 `sandbox_apply: Operation not permitted`로 exit 71이고, `webhook_oversize`는 worker runtime probe가 같은 메시지로 실패함을 직접 확인했다. 격리를 끄거나 테스트를 건너뛰는 변경은 하지 않았다. 추가로 시도한 CLI 전체 테스트 target lint는 기존 테스트의 `unwrap_used` 경고 때문에 통과하지 않았다. 이 회차의 실행 환경으로 worker 전체·외부 provider·실배포 검증을 대신할 수 없다.

main 통합 뒤 `python3 tools/check_plan_docs.py`는 `markdown files checked: 171; errors: 0`을 출력했고, `aip-ir codes` 8개와 CLI 계약 골든 4개도 다시 통과했다.

문서는 SDK·운영·wire 오류·결정 기록·CLI 출력 안내를 갱신했다. 초기 설계 대조와 개발 경험 비교는 과거 표본의 관측이라는 점을 명시하고 PoC 파일 확장자 링크를 고쳤다. 문서 전체의 내용 감사를 완료한 것은 아니다. CD-1 SafeNumber 단일 wire·시퀀스 상한, migration `requiresReview` 강제, 구조 정리와 예제·AI 평가 후속은 이 회차에 구현하지 않았다.

## 운영 문제 점검 회차 (2026-10-13)

worker 격리, 실배포 미검증 항목, 낮은 심각도 보안 항목, 의존성을 점검한 회차의 기록이다. 항목별 수정 전 실패·조치·커밋은 [보안 감사](../plan-docs/reviews/2026-10-13-security-audit.md)의 S-34 이후 행과 6.2절에 있다.

| 방금 실행한 명령 | 확인한 범위 |
|---|---|
| `node product/tools/proxy-check.mjs` (Caddy 2.11.7, 자체 서명 인증서, 임시 schema의 실제 서비스) | 16개 통과: Origin·CORS, Authorization·본문, `x-request-id` 유지·생성, `X-Forwarded-*`, 1MiB 초과, SSE keepalive 무버퍼, WebSocket Origin 필수·101·첫 프레임 인증 |
| `cargo test -p aip-auth --lib idp_tests` | mock HTTPS IdP: discovery의 issuer·jwks_uri, 겹침 회전, kid 교체, 폐기된 kid, 위조 kid, IdP 장애 중 캐시 키, JWKS 파일 변경 감지. 실제 IdP는 확인 못함: 계정 없음 |
| `AIP_BACKFILL_ROWS=12000000 cargo test -p aip-migrate --test large_backfill -- --ignored --nocapture` | 합성 1200만 행: 이전 방식은 20초 statement 한도에서 취소되고 읽기를 20.06초 막음. 상수 backfill은 적용 0.81초, 최대 읽기 대기 0.76초. 1200만 행 Text→Int 변환 4.5초 |
| `cargo test -p aip-migrate`, `--test online_backfill` | 테이블 재작성·행 갱신 없음, 적대적 값 인용, 잠금 대기 2초 뒤 `DB_LOCK`과 같은 digest로 재적용, 순환 migration, UTC 변환 |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --lib` | worker 후손 프로세스 정리(손자 3종), macOS 샌드박스(읽기·쓰기·fork·spawn·network), Node 실행. 이중 fork 데몬 테스트는 한계를 기록한 `#[ignore]` |
| `bash tools/dependency-check.sh` | `cargo audit`·`cargo deny`를 10개 manifest에, `npm audit`를 SDK에 실행해 모두 통과(`rsa` RUSTSEC-2023-0071은 `deny.toml`의 근거 있는 예외) |

미실행: 실제 외부 IdP, nginx, 두 번째 PG 인스턴스·다중 서버 인스턴스.

## Python 호출 SDK 회차 (2026-10-13)

| 방금 실행한 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `python3 -m unittest product.tests.google_storage_provider_test product.tests.python_sdk_test` | 11 passed | 계약 지문 header·불일치, 응답 유실 뒤 같은 key 재전송, 재시도의 실행 전 거부를 미확정으로 유지, 세션 principal 범위, 관리 경로 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed | 실제 JWT·PostgreSQL 서비스에서 worker 없음·Node·Python worker × decimal·safe wire로 Python SDK의 발견·읽기·TS 쓰기의 같은 key 재생·`/status`·지문 고정 거부, worker 실행에서 operation·job |

## 백엔드 기능 포괄성 보완 회차 (2026-10-13)

job의 업무 데이터 접근, schedule·webhook·outbox trigger, health probe를 추가한 회차의 기록이다. macOS, 로컬 PostgreSQL에서 실행했다. 같은 시간에 다른 작업자가 같은 PostgreSQL에서 검증을 돌렸을 수 있다. 첫 v6 실행에서 `extension_lifetime`의 worker PID 기록 시간 초과가 한 번 났고, 단독 재실행 3회와 이후 전체 실행 2회에서는 통과했다. 부하에 민감한 기존 타이밍 테스트로 보고 기록만 남긴다.

| 방금 실행한 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true bash tools/verify.sh` | Rust 599 passed, 2 ignored, 0 failed. Node 105 passed, 0 failed | root workspace(새 cron·서명 단위 테스트 포함 안 됨), V1/V2/V3/V5 suite, 제품 lint·fmt, SDK package |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked --target-dir target --no-fail-fast` | 52 passed, 0 failed | worker ctx 변경(`ctx.data.read`, `apply` key) 뒤 기존 Node/Python 회귀 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --no-fail-fast` | 114 passed, 0 failed | 신규 `job_data`, `triggers`, `outbox`, `health`, cron·서명 단위 테스트와 기존 transport 전체 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed | 재배치 binary·설치 SDK·두 wire·Node/Python worker |

신규 e2e가 실제 PostgreSQL에서 확인한 것: 행 정책으로 숨은 행에 대한 job 전이가 `WIRE.MISSING_TARGET`, 선언 없는 전이는 job 실패, 같은 key 재호출과 commit 뒤 worker crash 재시도에서 전이를 다시 실행하지 않고 journal 재생(Node·Python). webhook 정상·재전송·위조·무서명·주체 비활성(503). 두 instance가 같은 schedule slot과 같은 outbox 행을 경쟁해도 job 하나. 배포 digest 변경 뒤 `/ready` 503. 독립 컨텍스트 검토가 지적한 서명 밖 header key, 서비스 주체 보존 quota, outbox head-of-line, DST 반복 시간, grace 하한, `/ready` pool 사용, outbox id 재사용 7건을 고친 뒤 위 명령을 다시 실행했다. 다른 작업자의 커밋(`826028f`까지) 위로 rebase한 뒤 같은 명령을 다시 실행해 verify Rust 635 passed·2 ignored·0 failed, Node 108 passed, v4 53 passed, v6 114 passed, smoke 1 passed를 확인했다. AWS·GCP·실제 provider webhook 전송, Linux worker에서의 신규 e2e는 실행하지 않았다.

## resource 독립 operation 회차 (2026-10-10)

이 절은 2026-10-10 resource 독립 동기 operation 회차의 고정 검증 기록이다. 당시 기준 문법은 [EXECUTION](EXECUTION.md)에 적었다. 그 뒤 추가된 durable job, managed file/event, HTTP broker의 현재 계약은 [관리형 durable execution](MANAGED-EXECUTION.md)을 따른다. 이 절의 명령 결과를 새 변경의 실행 증거로 간주하지 않는다.

| 방금 실행한 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true bash tools/verify.sh` | Rust 487 passed, Node 72 passed, 실패·ignore·skip 0 | root workspace와 V1/V2/V3/V5 독립 suite, 제품 lint·fmt, SDK package, 마지막 CLI build. Rust 수치는 제품만의 테스트 수가 아님 |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked --target-dir target` | 34 passed, 0 failed/ignored | 공통 worker 호출 리팩터링 뒤 기존 Node/Python·framing·ctx·읽기/쓰기·기한·wire 회귀 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target` | 43 passed, 0 failed/ignored | 새 operation 네 조합, 기존 HTTP·인증·CORS·DB 기한·배포/WRITE 복구·세션 결과 보존 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed, 0 failed/skipped | 재배치 binary·설치 SDK strict 소비자, 실제 JWT·owned PG schema, Node/Python × safe/decimal operation·Unicode·범위 거부·타입 음성 대조 |
| root와 수정한 다섯 spike의 `cargo fmt … --check`, `bash -n tools/verify.sh`, `node --check product/tools/smoke.mjs`, 문서 상대 링크 검사 | exit 0 | 포맷·shell/JS 문법·로컬 문서 링크. 외부 링크 가용성 보장은 아님 |

먼저 재현한 실패는 실행 중 principal revoke 뒤 output 반환, safe 배포에 decimal 설정으로 큰 actor 등록, 안전 정수 초과 operation 입력 수락, SDK에서 표현할 수 없는 범위 metadata 수락이다. 수정 후 같은 음성 대조를 포함해 위 suite를 실행했다. revoke 테스트는 worker가 시작 파일을 쓴 것을 확인한 뒤 폐기해 수락 이전 거부와 구분한다. 기존 `principal` 이름의 실제 resource와 새 관리형 mode의 전환도 migration에서 차단한다.

첫 설치 smoke는 기존 CLI 실행 파일의 오래된 `dependencies: []` 생성 계약과 새 SDK 타입이 달라 실패했다. 현재 소스로 CLI를 build한 뒤 같은 pack/install/typecheck/runtime 검사가 통과했다. package 도구가 현재 source에서 binary를 빌드했다는 provenance를 보장하는 것은 아니므로 실행 순서를 SDK 문서에도 명시했다.

독립 Sol 검토의 실행 중 폐기·정수·배포 wire 지적을 부모가 실제 테스트로 재현해 보완했다. 마지막 재검토는 소스/문서 대조이며 별도 테스트 실행으로 합산하지 않는다. Luna는 legacy identity·패키징과 다음 Job의 PoC 재사용 경계를 읽기 전용으로 검토했다. 이번 회차 Claude Code의 새로운 독립 실행 검토는 미실행이며 과거 감사의 상호 검토 기록과 구분한다.

당시 미실행/미구현: Linux worker 격리, 동기 요청 disconnect의 프로세스 회수 실측, durable Job·파일·외부 효과·전체 tenant 경로·운영 provider 연결·부하/성능 측정. 업무 테이블 0개 실행은 운영 PostgreSQL이 없는 배포를 의미하지 않는다. durable 회차는 아래와 같이 별도 검사해야 하며, provider 실서비스 exactly-once나 Linux filesystem 격리로 확대 해석하지 않는다.

## 관리형 durable execution (2026-10-11)

현재 구현 계약은 [MANAGED-EXECUTION](MANAGED-EXECUTION.md)에 정리했다. 아래는 macOS arm64와 로컬 PostgreSQL에서 현재 source로 실행한 결과다. Cargo 명령에는 `CARGO_NET_OFFLINE=true`, `CARGO_TARGET_DIR=target`을 적용했다.

| 실행 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `bash tools/verify.sh` | exit 0, `AIP core verification passed` | workspace 테스트·제품 lint·SDK 검사·v1/v2/v3/v5 독립 suite·마지막 CLI build. 새 Job/file/event/HTTP store, durable principal, migration upgrade, SDK Job 회귀 포함 |
| `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked` | exit 0, 실패 0 | 기존 worker 회귀와 Node/Python managed context, 선언 capability 및 안정 key 검사 |
| `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked` | exit 0, 실패 0 | 전체 HTTP transport 회귀와 새 durable 통합 시나리오 4개. 재기동·취소·owner 격리·HTTP 불확실성·중첩 예산·자식 실패 재생을 두 언어에서 검사 |
| `node product/tools/package.mjs --out /tmp/aip-managed-release-20261011-b` | `ok:true`, 파일 8개 | 현재 CLI, 설치 SDK tarball, Node/Python 예제와 source manifest 조립 |
| `node product/tools/smoke.mjs --bin <artifact>/bin/aip --sdk-package <artifact>/sdk/aip-sdk-0.1.0.tgz --workers node --extension-dir <artifact>/extensions` 및 `--workers python` | 두 명령 exit 0, 각 `ok:true` | 설치한 SDK strict 타입 검사와 실제 JWT/PG 실행. Node/Python × decimal/safe 네 조합에서 Job `SUCCEEDED`, Unicode 길이 3, 같은 key의 같은 id 재생. 기존 read/cache/write/revoke·동기 operation도 검사 |
| artifact에서 `shasum -a 256 -c SHA256SUMS`, source root에서 `shasum -a 256 -c <artifact>/SOURCE_SHA256SUMS` | artifact 8개 및 source 200개 일치 | 로컬 무결성. 공개 배포 서명이나 commit provenance 보장은 아님 |
| root 및 수정한 여섯 spike의 `cargo fmt … --check`, `bash -n tools/verify.sh`, `node --check product/tools/smoke.mjs`, 문서 상대 링크 검사 | exit 0, 문서 17개 중 누락 링크 0 | 포맷·문법·로컬 문서 연결 |

검증 중 순수 operation이 금지된 context 호출 오류를 잡고 성공을 반환할 수 있는 회귀를 기존 음성 테스트로 확인해 차단했다. 설치 소비자의 선택적 Job API와 smoke 결과 타입도 strict 검사에 맞췄다. 저장소 이동으로 드러난 v12 테스트의 개발자 절대 경로는 `CARGO_MANIFEST_DIR` 기준으로 바꾸고 해당 타입 테스트와 전체 v6 suite를 다시 실행했다. 이동 전 경로가 남은 로컬 crate 빌드 캐시는 재생성했다.

HTTP 통합 검사는 통제된 loopback provider fixture를 사용한다. 같은 안정 key 사이에 다른 capability 호출을 끼워도 provider mutation이 한 번만 전송되는 경우, 응답 유실 후 `WIRE.EFFECT_UNKNOWN`, 전송 중 취소 후 `WIRE.CANCELLATION_UNCONFIRMED`를 확인했다. 이 결과는 실제 운영 provider의 exactly-once 보장이 아니다. 실시간 경로는 cursor polling이며 SSE/WebSocket은 미구현이다. 외부 IdP·운영 provider·대용량 object storage·Linux worker 격리·부하 및 성능 측정은 미실행이다.

## 첫 공개 뒤 반복 검증 회차 (2026-10-06)

이 회차는 기존 구현의 재검증과 입력 경계·빌드 안내 보완이다. caller 표현이나 서버 권한, 최종 문법·wire 선택을 변경하지 않는다. 이전 실행 묶음은 당시 source 기록이며 이번 source 변경을 포함하지 않는다.

| 명령/실행 | 결과 | 확인한 범위 |
|---|---|---|
| `CARGO_NET_OFFLINE=true bash tools/verify.sh` | Rust 230 passed, Node 53 passed, 실패·skip 0 | 전체 workspace 테스트, SDK cache/session/pending/contract 단위 테스트, offline 설치 SDK, 전체 fmt, 실행 코드와 제품 3 crate lint, 마지막 제품 build |
| `cargo test -p aip-cli --test service_boundaries --locked --offline` | 5 passed, 0 failed | FIFO를 정의·설정·migration·DB CA로 넣었을 때 유한 시간 내 오류. 크기 제한·origin·출력 원본 보호 |
| `node --test product/tests/service-smoke.test.mjs` | 1 passed, 0 failed | 재배치 binary·설치 SDK, 두 wire, 실제 JWT/PG read/write/replay/revoke, Node/Python 확장 |
| node_modules가 없는 임시 checkout에서 `npm ci --prefix spikes/spike-0-ts --offline --ignore-scripts --no-audit --no-fund` 후 SDK build | 두 명령 exit 0 | 기존 설치 디렉터리를 공유하지 않고 lockfile과 npm cache로 compiler·타입 의존성을 준비 |
| `actionlint .github/workflows/verify.yml`, `bash -n tools/verify.sh` | exit 0 | workflow 문법과 shell 문법 |
| SDK compiler가 없는 임시 checkout에서 verify script 실행 | exit 1, 설치 명령 출력 | 검사 누락을 성공으로 보고하지 않는 음성 대조 |
| 각 독립 `Cargo.toml`의 `cargo test --manifest-path … --locked --offline` | V1 43·V2 33·V3 5·V4 32·V5 22·V6 40·V7 1·V11 9·prototype 37, 실패·ignore 0 | 정의 상한, 실제 SQL 정책, 쓰기·worker 원자성, 요청/DB/worker 기한, 응답 유실 복구, CORS·두 wire·Node/Python 확장 |
| [GitHub Linux CI #37482679158](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/37482679158), commit `06ff8d8` | success、Rust 230 passed、Node 53 passed、실패·skip 0 | Ubuntu x86_64, Rust 1.99.0, Node 26, PostgreSQL 17, 새 checkout 의존성 설치와 core 검증 |

FIFO 회귀는 수정 전 `waited for a FIFO writer`로 실패했다. SDK 회귀는 수정 전 `BUILD_FAILED`와 기대한 `SDK_TOOLCHAIN_MISSING`이 달라 실패했다. 두 반례를 확인한 뒤 해당 동작만 보완했다. 별도 9개 파일의 포맷 변경은 변경 전 source를 rustfmt한 결과와 바이트 단위로 대조했다.

V7의 최초 `--locked` 실행은 오래된 lockfile 때문에 테스트 시작 전에 실패했다. 현재 V2 읽기 엔진의 TLS 의존성 39개를 기록하되 기존 locked 패키지 버전은 제거·교체하지 않았다. 갱신 뒤 V7의 `--locked` 검사는 1 passed다. 이 회차 Rust 합계 452개는 workspace 230개와 독립 경로 222개의 합계이며, 제품 서비스 452개로 표현하지 않는다. Node 53개와 제품 smoke 1개도 서로 다른 검증 범위다.

Root `cargo clippy --workspace --all-targets --offline -- -D warnings`는 기존 CLI 통합 테스트의 `unwrap_used` 진단으로 실패했다. 실행 코드 `--lib --bins`와 제품 3 crate `--all-targets` 검사는 exit 0이다. 전체 테스트 lint 통과로 확대하지 않는다. Linux CI는 위 실제 실행 결과로 확인했으며 macOS worker 및 외부 운영 배포 검증을 포함하지 않는다.

보완 목록과 자동 검증의 경계는 [VALIDATION](VALIDATION.md)을 따른다. 아래 표와 bundle 해시는 이전 제품 조립 회차의 기록이다.

## 현재 제품 경로의 실제 검증

| 명령 | 결과 | 확인한 범위 |
|---|---|---|
| `cargo test -p aip-auth -p aip-migrate -p aip-service --offline` | 39 passed, 0 failed | JWT/JWKS/TLS·actor 연결, DDL 계획·변환·backfill·rollback·동시 적용·drift·wire 원자성·이관 재시도 |
| `cargo test -p aip-cli --test service --test service_boundaries --test service_flow --test adoption_flow --offline` | 8 passed, 0 failed | 실제 JWT read/apply·재기동·live 정책 변경 거부·토큰 폐기와 재활성화·기존 서버 쓰기 결과 이관·설정 및 출력 보호 |
| `node --test product/tests/sdk-package.test.mjs product/tests/service-smoke.test.mjs` | 5 passed, 0 failed | offline SDK 설치·strict 타입 음성 대조·JWT·캐시·쓰기·멱등 재생·공백 경로 재배치·Node/Python 확장 |
| `cargo clippy -p aip-auth -p aip-migrate -p aip-service --all-targets --offline -- -D warnings` | exit 0 | 제품 조립 crate·인증·마이그레이션과 해당 테스트 경고 없음 |
| `cargo build -p aip-cli --offline` | exit 0 | root CLI 실행 파일 생성 |

재배치 확인은 [최종 묶음](dist/2026-10-06T11-19-25-091Z/README.md)의 binary·SDK tarball·확장 파일을 저장소 밖 공백 경로에 복사해 실행했다. 기본 설정·Node·Python 각각 decimal/safe 두 wire에서 실제 JWT/PG 흐름을 실행한다. SDK 읽기·캐시 hit·쓰기 후 무효화·같은 키 재생·인증 철회를 확인한다. 확장 검사는 이미 적용한 Event에 `Event.confirm`을 호출해 count 0과 같은 키 재생을 확인한다. 새 행 변경과 확장 rollback의 엔진 검증은 아래 기존 worker/전송 회귀 범위와 구분한다.

묶음의 SHA256SUMS 8개 파일과 SOURCE_SHA256SUMS 181개 source 입력을 메인이 다시 비교했으며 mismatch 0이다. 마지막 `cargo build`의 binary와 묶음 binary도 바이트 단위로 일치한다. Cargo 테스트가 다른 dependency feature 조합의 실행 파일을 target 경로에 복원할 수 있어, 최종 build를 테스트 뒤 실행하고 묶음을 새로 생성했다.

## 함께 확인한 기존 엔진 회귀

2026-10-05 이어서 작업 중 `cargo test --manifest-path <각 Cargo.toml> --offline`을 실행했다. V1 43·V2 33·V3 5·V4 32·V5 22·V11 9·V6 40·prototype 37개가 각각 실패 0이다. 기존 root CLI의 contract_golden/golden/codes/diff 19개도 실패 0이다. 이 숫자는 현재 제품 47개에 합쳐 제품 표면 전체를 보증하는 수치로 사용하지 않는다.

원격 DB TLS 연결의 오류 타입 변경에 맞춰 기존 V4/V6 테스트의 진단 분기를 갱신했다. 운영 JWT의 4KiB 한도에 맞춰 V6 기존 Bearer 경계 테스트를 공개 MAX_TOKEN+1로 바꿨다. 실제 오류·경계 검사 자체를 제거하지 않았다. V1/V2/V6과 제품 crate의 수정 파일 포맷을 확인했다. root CLI 전체 fmt 검사는 이번 작업에서 수정하지 않은 encrypt_e2e.rs의 기존 포맷 차이를 보고하므로, root CLI는 이번 수정 파일을 직접 rustfmt 검사했다.

## 독립 검토와 제한

Sol high가 별도 문맥에서 운영 중 구 정책 실행, 폐기 토큰 재활성화, 없는 신원 폐기 성공, 이관 멱등 namespace 누락, wire 고정의 별도 트랜잭션 문제를 발견했다. 메인은 실제 RED/GREEN과 코드로 대조했다. 최종 Sol 검토는 같은 SQL 연결의 배포 잠금·wire 포함 승인 지문·원자적 metadata 기록·폐기 cutoff를 읽고 추가 필수 결함을 확인하지 못했다. 마지막 독립 검토에서 DB 테스트를 재실행한 것은 아니다.

검증 환경은 현재 macOS arm64, 로컬 PostgreSQL, 통제된 HTTPS 인증서 fixture와 메모리 생성 RSA 키다. 외부 실제 IdP 계정·TLS reverse proxy 배포·다른 OS·대형 테이블 무중단 변경은 미실행이다. actor resource 교체·임의 SQL 변환·자동 역방향 migration·구버전 client 공존은 지원하지 않는다. 최종 작성 형식·Id 표현·쓰기 조합·라이선스의 미결정을 승인으로 승격하지 않는다.

실행 절차는 [OPERATIONS](OPERATIONS.md), 설계 관문은 [서비스](../crates/aip-service/README.md)·[인증](../crates/aip-auth/README.md)·[마이그레이션](../crates/aip-migrate/README.md)·[정의 한계](../spikes/spike-v1-fixture/PRODUCTION-LIMITS.md)·[정책 실행 한계](../spikes/spike-v2-read/PRODUCTION-RUNTIME.md)를 따른다.

## 2026-10-23 WBS 7.2.10 인증 회원 SDK/API journey (실제 CI 서비스, 범위 slice)

**기준과 범위.** Backend 기준은 pinned revision `7948b7e…`, UI는 pinned revision `2e7c021fa5abebd72278d841b8066a33d45cfbb3`이며 dirty 원본 UI/backend checkout은 수정하지 않았다. 고정 UI의 여섯 API 파일에 있던 30 callsite/25 unique route inventory와 `ui-overlay` adapter를 대조한 뒤 아래의 모델·방문자 정책과 무관한 최소 회원 경로를 실제 실행했다.

| 경로/adapter 계약 | 실제로 수행한 일 | 판정 |
|---|---|---|
| `POST /auth/login`, `POST /auth/reissue` 1회 | dedicated fixture member 로그인, refresh cookie 회전, 새 access token 획득 | 성공 |
| SDK `extension` → `Video.homeVideoAll` (`createReadApi().getAllVideo`) | 로그인 principal로 고정 fixture video 목록을 읽음 | 성공 |
| SDK `read` + `bundle({atomic:[{create…}]})` + `apply` | 현재 `VideoBookmark` 확인, 본인 북마크 create 뒤 다시 toggle하여 `VideoBookmark.remove` | 성공, on/off 모두 단정 |
| SDK `bundle({atomic:[{create…}]})` + `apply` | 본인 댓글 create, `Comment.edit`, `Comment.delete`; 마지막 read가 member row policy에 따라 soft-deleted row를 숨기는지 확인 | 성공, 삭제 뒤 0 rows 단정 |
| `POST /auth/logout`, `POST /session` | refresh session 폐기 뒤 이전 access token 확인 | 성공, HTTP envelope `200`와 body `ok=false, code=WIRE.UNAUTHENTICATED` |

**SDK·서비스·fixture.** Linux GitHub Verify run [39851918809](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39851918809), exact code SHA `1033269ec1389ddd7b750b6ad9f5e53bd100a53b`에서 `bash tools/verify.sh`와 `bash scripts/verify-mysql-worker-e2e.sh run-ci`가 모두 통과했다. 보조 workflow `Linux worker jobs` 39851918812, `Linux cgroup v2` 39851918750, `Object storage` 39851918710, `App packages` 39851918717, `External brokers` 39851918739도 같은 SHA에서 success다(총 6/6). 통합 script가 real `aip service serve`, sandboxed Python extension worker, MySQL 8.4를 실행했고, workflow가 run-owned MongoDB와 Redis native fixture도 시작했다. 이 회원 journey에는 기존 전용 fixture account와 그에 연결된 모델 없는 video 1개 및 Mongo `video_distribution` 문서 1개를 사용했다. Mongo의 감정 필드는 고정 fixture 값이지 모델 추론을 흉내 낸 결과가 아니다. 운영 계정·운영 데이터·외부 영상 수집은 쓰지 않았다.

**실제 결과.** `facereview_accounts_log_in_refresh_and_log_out_without_the_originals_replay_defect`가 `1 passed; 0 failed`(22.41s)다. 그 stdout에는 다음이 출력됐다: `member SDK journey: login, one refresh, UI adapter video read, bookmark on/off, comment create/edit/delete, logout`. 전체 end-to-end script의 나머지 test도 통과했고 Verify workflow conclusion은 `success`다. 새 raw log는 `/Users/winterholic/.ttakkari/data/outbox/5b966616-0637-43fc-9a2c-938c348bc032/aip-verify-39851918809-raw.log`(13,859 lines, 2,170,057 bytes)다. 로그 검사 결과 JWT 형태 문자열 0, Authorization bearer 0, refresh cookie 0, fixture password 표식 0으로 민감 인증값 출력은 발견되지 않았다.

**발견과 수정.** Adapter fake tests는 실제 SDK surface를 검증하지 못했다. real SDK journey가 `apply`로 create를 보내던 bookmark/like/comment adapter가 contract를 어긴 것을 드러내 `bundle()`의 기존 `{atomic:[{create, values}]}` envelope로 옮겼다. D-A26이 보장하는 단일 atomic write 실행과 같은 create field/`allow`/row policy를 유지하며, `facereview.aip`의 `VideoBookmark`·`Comment` owner 조건을 통과하는지 real member request로 확인했다. 삭제 댓글은 `.aip`의 `Comment rows read when ... is_deleted = false ... or isAdmin(actor)` 정책 때문에 member에게 읽히지 않는다. 그래서 시험은 이전 값 `is_deleted=true`를 읽는다고 가정하지 않고, 명세가 요구하는 `rows.length === 0`을 단정한다(관리자 예외 조건도 정의에 남아 있다).

첫 성공하지 못한 동일-SHA 시도인 Verify 39848380032는 journey를 logout 뒤까지 실행했고, `/session` 본문에서 `WIRE.UNAUTHENTICATED`를 받았으나 시험이 이를 HTTP 401로 비교해 실패했다. 기존 `mysql_facereview_auth.rs`도 `/session`의 body `code`를 검사하며, AIP endpoint는 이 application error를 HTTP 200 envelope에 담는다. 수정 SHA `1033269…`의 journey는 status 200, `ok=false`, `code=WIRE.UNAUTHENTICATED`를 모두 단정하고 통과했다. 이는 기대를 느슨하게 하지 않고 실제 응답 envelope와 기존 권한 의미를 확인한 것이다. 이 첫 실패의 raw log는 `/Users/winterholic/.ttakkari/data/outbox/5b966616-0637-43fc-9a2c-938c348bc032/aip-verify-39848380032-raw.log`에 보존했다.

**검증 경계와 재개.** TypeScript와 Vite client bundle은 이전 UI build slice에서 통과했고, preview browser에서는 SPA shell만 확인했다. 이번 실제 auth/member journey는 Linux CI에서 실행한 API/SDK 테스트이며 로그인한 UI browser journey는 실행하지 않았다. Browser/prerender는 미검증이며 25개 route 전체 화면 동등성도 확인하지 않았다. Mac nested Codex의 알려진 Python worker probe 차단(`sandbox-exec: sandbox_apply: Operation not permitted`)은 이전 환경 증거로만 남기고 이번에 다시 시도하거나 우회하지 않았다. ML model 권리·로드·추론, 방문자 policy, 익명 principal은 이번 slice 밖이고 판정하지 않았다. 공개 `VideoViewLog.create` 계약은 바꾸지 않았고 internal extension 경계도 풀지 않았다. WBS 7.2.10 전체나 ML/WBS 완료 표시는 하지 않는다.

## 2026-10-23 WBS 7.2.10 React browser member journey (producer mismatch 재개)

**기존 실패 증거.** Verify `39872659856`, SHA `0e3b9d92d8cbfe8b04cf2f6ffb17ea4fb5a2fde7`의 기획 직접 확인 결과는 로그인·`/session` 성공 뒤 `/read` 2건 HTTP 200 `ok=false`, `WIRE.CONTRACT_MISMATCH`였다. 이 실패를 browser 성공으로 세지 않는다. 앞서 `--read-only`를 제거한 수정도 같은 오류로 실패했으며 재사용하지 않았다. 이전 report와 safe browser JSON은 `aip-7.2.10-browser-journey-report.md`, `fix-run-39872659856-browser/add/add-browser-result.json`에 있다.

**producer 대조(코드 + 로컬 CLI, CI browser 값과의 대조 전).** `mysql_facereview_auth.rs`는 서버 정의 파일에 `facereview.aip`와 `facereview-auth/operations.aip`를 결합하고, SDK binding도 그 파일에서 생성한다. 반면 overlay script는 인증 operation 두 개가 없는 relational fixture만 binding 생성에 사용했다. 이 경로를 로컬 CLI로 생성해 본 값은 relational 입력 SHA256 `7872ee9eafc7f2e88b0e0dbc6861917c797b351a4fbb3f9f6a4bbc34074a4900`, fingerprint `2c24f6f10a0cd60820c9911046f031e23da94df2c90ce8a7361df30d376aab56`; 합성된 실제 서버 입력 SHA256 `b03f938989a5407ac39d113dbbcd771e87c88f90884991d55530fadc14f7db5f`, fingerprint `cab5e5ff77cd04117f176035d913843907904d9ced810d1c53764b76c5536ce2`다. 따라서 이 producer 입력 차이가 일치하지 않는 계약을 만들 수 있다는 강한 재현 증거는 있으나, 이전 CI가 실제로 보낸 header 값과의 동일성은 아직 확인되지 않았다.

**수정 및 fail-fast 회귀검사(실행 대기).** CI가 두 fixture를 한 번 합성한 definition 경로를 overlay generator와 MySQL/Rust browser journey에 함께 넘기도록 했다. Rust 여정은 이를 컴파일 시 fixture 결합 바이트와 SHA로 비교하고, generated binding fingerprint와 실제 listening fingerprint를 확인한다. 이어 UI에 있는 binding fingerprint도 browser 시작 전에 검사하며, 파일 경로·SHA·fingerprint가 오류에 표시된다. safe `contract-evidence.json`에는 fixture/definition/binding SHA와 server·generated·UI fingerprint를 기록하고, 두 browser result JSON은 실제 각 API 요청의 `x_aip_contract` 값과 상태를 보존한다. 계약 지문은 기록하지만 비밀번호·bearer·cookie는 기록하지 않는다.

**수정 SHA 실행 결과.** `e35113a73195a0ff1c7df43b41810eed2b8c1c97`의 [Linux Verify run 39875726918](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39875726918)은 전체 workflow `failure`다. 단계 10 `bash tools/verify.sh`와 단계 11 pinned UI 준비는 success였고, 단계 12 `Verify real HTTP to Node and Python workers to MySQL`가 failure로 끝났다. 나머지 같은 SHA 보조 workflow 6종(Linux worker jobs 39875726847, Linux cgroup v2 39875726901, Database contracts 39875726911, Object storage 39875726921, App packages 39875726881, External brokers 39875726876)은 모두 success다.

browser artifact는 이번 Run outbox `ci-39875726918/`에 보존했다(`contract-evidence.json`, `add/add-browser-result.json`, `add/add-failure.png`). 세 definition 경로(SHA256 `b03f938989a5407ac39d113dbbcd771e87c88f90884991d55530fadc14f7db5f`)가 정합하고, generated binding·UI binding은 SHA256 `f6a25a96ba74a900ccbe17163f001bfa945ba0a9d0a0f58e7a0b0283643a2d86`, fingerprint `cab5e5ff77cd04117f176035d913843907904d9ced810d1c53764b76c5536ce2`였다. service listening fingerprint와 browser의 실제 `/read` 요청 `x-aip-contract`도 같은 `cab5…6ce2`이며 두 `/read` 모두 HTTP 200, `ok=true`, `contract_matches_runtime=true`다. 이 실행은 이전 `WIRE.CONTRACT_MISMATCH`를 재현하지 않았다. 이전 실패 run의 실제 요청 header 값은 보존되지 않아 그 옛 값 자체를 사후 확정할 수는 없지만, 합성 definition 공유 뒤 실제 계약 요청이 일치하고 읽기에 성공했다.

새 실패는 계약 단계 뒤다. 로그인과 `/session`은 HTTP 200 `ok=true`이고 여러 `/extension`도 HTTP 200 `ok=true`지만, UI는 홈 화면에 도착한 뒤 fixture 영상 카드를 찾지 못해 `add` phase에서 중단됐다. screenshot은 카드가 없는 홈 화면을 보인다. safe browser JSON에는 `/apply` 요청이 없고 두 번째 `remove` phase artifact도 없으므로 bookmark on/reload, MySQL 1행, off/reload, MySQL 0행은 검증되지 않았다. artifact는 `/extension` 응답 payload의 비밀 없는 shape나 구체적인 Playwright assertion을 담지 않아 UI list 불일치의 원인은 미확정이다. `gh` raw job log는 terminal escape safety guard 지침에 따라 가져오거나 우회하지 않았다. 현 시점에서 추가 추측 패치는 하지 않는다.

다음 확인은 기존 실행 원칙을 지켜 UI의 요청별 응답 shape와 화면의 list 상태를 비민감 계측으로 얻은 뒤 root cause를 확정하는 것이다. 지금까지 모델 추론·방문자 정책·로그인 벽은 바꾸지 않았다. WBS 7.2.10 browser journey의 완료 기준(실제 React login → fixture member video → bookmark on/reload + DB 1행 → off/reload + DB 0행)은 미달이다.

### 후속 결과: Verify `39885042974` (2026-10-23, `b504f62161fb9e3065ac9da9972ebe9928e0c315`)

앞 절은 후속 실행 이전의 시점 기록으로 보존한다. 해당 SHA Verify와 Linux worker jobs `39885042938`, Linux cgroup v2 `39885043079`, External brokers `39885043135`, Object storage `39885043216`, App packages `39885042984`가 모두 success다. Verify 단계 10 product/runtime checks, 단계 11 pinned UI 준비, 단계 12 실제 HTTP→Node/Python worker→MySQL 및 browser journey가 성공했다. 브라우저는 전용 fixture 회원과 영상만 사용했다. API mock은 없고 외부 영상 provider는 요청하지 않았으며 fixture 썸네일 URL 하나만 loopback HTTPS 이미지로 전달했다.

**목록 root cause와 최소 변경.** 실제 `/extension` 응답과 `overlayreadAdapter.ts` 변환 결과에 fixture가 있었고, pinned UI의 `src/hooks/useAvailableVideos.ts`는 `Image()` thumbnail probe 실패 영상을 목록에서 제거한다. CI가 YouTube 이미지를 차단해 fixture까지 필터됐다. 제품 fallback/selector 완화 없이 테스트 하네스에서 고정 fixture 썸네일만 local HTTPS로 제공했다. 성공 artifact는 `Video.homeVideoAll=1`, `Video.homeCategory=2`, `Video.homePersonalized=1`; adapter 목록 길이도 각각 1/2/1; `.video-item-container` 총 2개, fixture 일치 2개, unhandled page error 0을 기록한다. `add-member-video-list.png`는 fixture 카드를 보여 준다.

**회원 bookmark journey.** pinned React UI revision `2e7c021fa5abebd72278d841b8066a33d45cfbb3`에서 로그인·회원 목록·fixture 카드 클릭 뒤, 첫 진입 안전 안내를 UI 자체 버튼으로 닫고 bookmark on/reload를 실행했다(`aria-pressed=true`). remove phase에서 off/reload(`aria-pressed=false`)를 수행했다. 두 phase의 실제 `/apply`는 HTTP 200 `ok=true`. `crates/aip-cli/tests/mysql_facereview_auth.rs`는 브라우저 전 count `0`, add 뒤 `1`, remove 뒤 `0`을 전용 fixture user/video key로 각각 SQL 조회 후 `assert_eq!`한다. 실행 코드가 두 React phase와 DB 조회를 이 순서로 잇고, 해당 CI 전체 success로 assertion 통과를 확인했다. 현재 browser artifact에는 DB count가 별도 JSON으로 출력되지 않아, 0→1→0은 CI 통과와 실행 코드의 결합 근거이지 artifact 직접 기록은 아니다.

**계약 및 남은 관찰.** `contract-evidence.json`에서 server/UI definition SHA `b03f…7db5f`, UI/generated binding SHA `f6a25a…2d86`, server/generated/UI fingerprint `cab5…6ce2`가 일치한다. phase JSON의 실제 `/read`, `/extension`, `/apply` `x-aip-contract` 값도 같은 fingerprint다. response sanitizer가 `/read` resource를 `unrecognized`로 남기며 각 phase에 일부 HTTP 200 envelope `WIRE.UNKNOWN_KEY`가 기록됐다(add 2, remove 2); console error는 add 2/remove 1이다. artifact로 요청 키를 안전하게 특정할 수 없어 이 부수 오류는 미해결이며 bookmark/카드 root cause에 연결하지 않았다. unhandled page error는 0이다.

safe evidence bundle은 이번 Run outbox `ci-39885042974/` (`contract-evidence.json`, `add/{add-browser-result.json,add-member-video-list.png,add-bookmarked.png}`, `remove/{remove-browser-result.json,remove-member-video-list.png,remove-bookmark-removed.png}`)이다. 이번 검증은 요청한 React bookmark journey 범위에 한정되며 AIP 7.2.10 전체나 ML 추론 완료로 세지 않는다. 모델 추론·방문자 정책은 다루지 않았다.

### 후속: `/read Comment` envelope와 console 오류 진단 (2026-10-23)

진단용 변경 SHA `4c24eb6a2fb206d5598cf2841175bcdd8dc8490e`의 Verify `39888567142`는 strict browser `/read` envelope gate가 `ok=false`를 검출해 failure로 종료했다. safe artifact의 실제 React 요청 2건은 `POST /read`, HTTP 200, envelope `ok=false`, `WIRE.UNKNOWN_KEY`, resource `Comment`, operation `read`, envelope key `query`, rejected query key `traverse`였다. 읽기에 사용한 contract fingerprint와 server runtime fingerprint가 모두 `cab5…6ce2`라 contract mismatch가 아니다. login/session 및 `VideoBookmark` read, bookmark `/apply`는 성공했다. auth refresh 요청의 거절도 아니다.

호출 코드는 pinned React adapter의 `watch.ts:getVideoComments`; 잘못된 호출은 root `traverse`를 보내고 `sort`에 `direction`을 사용하며, relation selection에는 Comment definition이 공개하지 않은 User `profile_image_id`를 요청했다. SDK 공개 nested-select 사례와 fixture definition의 `traverse user { select id, name }`에 맞춰 root traverse를 없애고 `select`에 nested user id/name을 넣으며 sort 키를 `dir`로 바꾸는 단일 query builder 및 회귀검사를 추가했다. UI는 watch 댓글 read에서 오류를 받으므로 댓글 fetch가 정상 동작하지 않는 범위다. fixture에는 Comment 데이터가 없으므로 화면에 댓글이 사라지는 것을 직접 증명하지 못했다. 북마크 경로와 API 계약 fingerprint에는 영향을 주지 않는다.

같은 artifact에는 `console_message_type=error`, `classification=network_resource_error`, `source=unmapped` 1건, unhandled page error 0건이 있다. raw console text는 보존하지 않았다. 진단 SHA의 safe evidence는 이번 Run outbox `ci-39888567142/facereview-ui-browser-39888567142-1/add/{add-browser-result.json,add-failure.png}`이다. 후속 CI는 network-failed 이벤트에서 allowlisted 외부 host class/resource type/error code만 기록해 이 console 오류가 의도적 외부 미디어 차단과 일치하는지 구분한다.

query builder 회귀검사는 14개 통과, browser safe-evidence 단위검사는 11개 통과했다. 이 기록 시점에서 최종 수정 SHA의 Linux Verify는 아직 실행 전이다. 전체 7.2.10, 25개 경로, prerender, ML/방문자 정책 완료로 확대하지 않는다.

### 후속 수정 검증: Verify `39891300600` (`4f5e6b0`)

동일 수정 SHA Verify는 전체 `failure`이며 단계 10 product/runtime와 단계 11 pinned UI 준비는 success, 단계 12 실제 HTTP/worker/MySQL/browser는 failure, safe artifact upload 단계 13 success였다. safe artifact `ci-39891300600/facereview-ui-browser-39891300600-1/add/`에서 login, `/session`, API fingerprint `cab5…6ce2`, video list extension 3종 응답/adapter에 fixture 포함, `Comment` HTTP 200 `ok=true`, bookmark `/apply` 2건 `ok=true`, `VideoBookmark` read 성공을 확인했다. `Comment` query에는 rejected root key가 없으며 recognized query keys는 `filter,limit,read,select,sort`다. sanitizer의 당시 중첩 select 계측이 nested `user` object를 unknown select 1개로 세었지만 서버 응답은 `ok=true`; 이후 safe JSON은 relation `user`의 allowlisted `id,name`을 별도 기록하도록 보완한다.

`add-member-video-list.png`에는 고양이 fixture 카드가 렌더되고 `add-failure.png`에는 watch route와 댓글이 보인다. 추가/새로고침의 UI bookmark 최종 상태, 두 번째 off phase, SQL count assertion 완료 여부는 safe JSON에 없고 Verify step12가 실패했으므로 아직 성공 판정할 수 없다. 실제 중단 시점과 버튼 상태를 안전하게 확인하기 위해 다음 artifact에는 allowlisted `journey_stage`, 예외 class, bookmark button visibility/aria-pressed만 기록하도록 계측한다. SQL 0→1→0은 이 CI 단계의 결과를 확인하기 전 판정하지 않는다.

console error는 add phase 2건, 각각 allowlisted failed request `host_class=youtube`, `resource_type=script`, `failure_code=net::ERR_FAILED`와 개수·분류가 일치한다. 이는 외부 media 차단 규칙으로 발생한 것이며, raw URL/text는 저장하지 않았다. `page_error_count=0`, API mock false, credential recording false, fixture 썸네일 redirect 1회 HTTP 200이다. 외부 영상은 차단 상태라 실제 재생은 범위 밖이다.

추가로 nested relation allowlist와 예외 class 기반 안전 artifact 검사를 넣었으며 Python evidence 검사 13개, UI adapter 검사 14개가 통과했다. 이 후속도 WBS 7.2.10 전체나 ML/model·visitor-policy 완료를 뜻하지 않는다.

### Reload 후 bookmark selector 진단: Verify `39893336376` (`eddb7ed`)

이 SHA의 전체 Verify는 failure이며 단계 10/11 success, 단계 12 failure, artifact 보존 성공이다. 새 safe `add-browser-result.json`에서 실패는 `journey_stage=bookmark_reload`, `failure_type=AssertionError`; `즐겨찾기 추가`/`즐겨찾기 해제` role locator 모두 absent로 기록됐다. 이 stage가 reload 호출 전후를 합쳐 놓아 현재 URL route와 실 DOM `aria-pressed` controls는 미확인이다. 따라서 이 JSON만으로 browser assertion의 정확한 원인을 확정할 수 없다.

그 외 로그인·session, list extension 3종, `Comment` read(두 번), `VideoBookmark` read, 두 `/apply`는 모두 HTTP 200 envelope `ok=true`; comment nested relation `user:{id,name}`의 unknown select count는 0이다. Add phase에서 실제 UI apply 이후 reload 지점까지는 도달했으나 bookmark toggle이 `bundle`을 쓰므로 이 `/apply`들은 SQL 북마크 쓰기 증거로 보지 않는다. `mysql_facereview_auth.rs`의 0→1→0 assertion 전체가 성공했는지도 run 실패 때문에 입증되지 않았다. DB 결과를 성공으로 쓰지 않는다.

이에 따라 다음 artifact는 `bookmark_reload`를 `bookmark_reloaded`/`bookmark_state_confirmed`로 분리하고, URL은 fixture route class로만, `[aria-pressed]`는 허용된 bookmark label과 true/false만 기록한다. session/login value와 arbitrary DOM/text는 저장하지 않는다. 같은 SHA의 console 2건은 차단된 YouTube `script`의 `net::ERR_FAILED`; 이는 의도적 외부 media 정책이고 page errors는 0이었다.

### 최종 browser failure-state 확인: Verify `39896024733` (`8d1a257`)

최종 변경 SHA Verify는 full `failure`; step 10 product/runtime과 11 pinned UI 준비 success, step 12 browser/worker/MySQL failure, step 13 safe evidence upload success였다. artifact `ci-39896024733/facereview-ui-browser-39896024733-1/add/`에는 login/session, fixture 목록 3 extension, actual card selection, watch bookmark on action 뒤 reload 및 응답들이 기록됐다. `Comment` read 2건은 `ok=true`, `rejected_query_keys=[]`, nested `user` field `id,name`과 `unknown_select_field_count=0`이다. Response/browser fingerprint는 `cab5…6ce2` 일치. 기존 `WIRE.UNKNOWN_KEY`는 해결됐다.

Reload 뒤 `journey_stage=bookmark_reloaded`, `failure_type=AssertionError`, route class `watch_fixture`, watch content visible. Role locator는 실패했지만 DOM allowlist 결과에서 `즐겨찾기 해제` `aria-pressed=true` 버튼이 있었고, `add` phase의 `add-failure.png`에는 플레이어 오류 modal이 가려져 있다. external failed requests는 `youtube` host class + `script` + `net::ERR_FAILED` 2건이며 console classification 2건과 일치, page errors 0이다. 따라서 현재 가장 근거가 있는 root cause는 테스트가 가리키는 북마크 상태 값이 아니라, 외부 video 차단으로 화면이 오류 modal을 띄워 accessible control을 덮는 것이며 이 인과는 network event와 screenshot에 근거한 판단이다.

이 modal의 pinned UI 확인 CTA는 `/`로 이동한다. role selector를 느슨하게 하거나 hidden DOM을 직접 조작하는 것은 허용된 검증이 아니므로 하지 않았다. `remove/reload` phase와 SQL 0→1→0 assertion은 실행 성공을 확인할 수 없어 미검증이다. fixture 외부 video를 풀지 않고 이 경로를 통과했다고 주장하지 않는다. 결과는 browser UI/DB journey 미완료로 남긴다. console 오류 분류 자체는 의도적 외부 provider 차단으로 확인됐다. 전체 7.2.10/25 paths/prerender/ML/model/visitor policy 완료는 아니다.

### Pinned React member profile read (2026-10-23, pending)

Code SHA `dd59658274e93c65bd32c9f30ff0506c35aab982` adds a separate real-browser profile phase to the Linux Verify workflow. At the fixture boundary, the dedicated member is seeded with the non-empty display name `AIP 프로필 표본회원` and distinct favorite genres `eating` and `horror`. The pinned React login populates its existing auth store from `getUserName`; the browser then follows the visible member route `/main` → `/my` → `/edit` and asserts the name and selected/unselected category controls through accessible UI locators. No UI feature or production policy is changed.

The phase records only allowlisted `/read` shape, status/envelope, contract-header match, and the fixed fixture values. It requires `User` to select the profile fields used by the original screen and filter by the authenticated user id, and `UserFavoriteGenre` to select `genre` and filter by that user. It uses the generated TypeScript SDK against the actual AIP service and the same dedicated MySQL fixture; browser API mocks are false. Credentials and response bodies are not retained. A screenshot and safe JSON result are uploaded under the run-owned `profile/` artifact directory.

Linux Verify run [39899325826](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/39899325826) was `pending` at its first query and `in_progress` at 2026-10-23 04:16 UTC. Therefore the profile screen and actual browser `/read` responses are **not yet verified**. The earlier SDK/API integration journey and separate bookmark runs remain as previously recorded and are not evidence for this new profile slice. The next action is to inspect this run's conclusion and, if complete, its safe browser artifact and raw step output. The profile scope remains open; no broader 7.2.10 completion is implied.

## 2026-10-23 WBS 7.2.10 profile write overlay and route coverage (code SHA `d29f1d372505e26fa8bec60062f0e6c036a771a3`, Verify pending)

**Implemented slice.** The pinned `/my/edit` already calls `updateProfile`, and the AIP definition already exposes `User.updateProfile`, `UserFavoriteGenre.remove`, and member-owned `UserFavoriteGenre.create`. The overlay now routes this call through `createWriteApi().updateProfile`: it reads only the signed-in member's genre rows, builds one atomic SDK `bundle()` containing the `User` patch and favorite-genre removes/creates, and uses one idempotency key. This preserves the existing server ownership checks and does not add profile validation rules. The browser fixture now edits the displayed name and replaces `horror` with `animal`, checks the `/apply` success and contract fingerprint, and the MySQL integration asserts the saved name and exact genre rows. A second member's fixed `vlog` row is seeded so the profile `/read UserFavoriteGenre` result verifies the server-side `rows read when user = actor` policy; the endpoint does not expose a caller `user` filter.

**Local checks.** Before implementation, the new adapter test failed because `updateProfile` was absent (`actual: undefined`, expected function). After implementation, `node --test porting/facereview/ui-overlay/tests/writeAdapter.test.ts` passed 5/5; `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` passed 15/15; the profile browser script AST check, `cargo fmt --all -- --check`, and `git diff --check` passed. These checks do not establish live service or browser success. No local MySQL/browser integration was run.

The code SHA `d29f1d372505e26fa8bec60062f0e6c036a771a3` was pushed to `main`. Verify run `39900453676` was cancelled at 2026-10-23 04:34:56 UTC during core step 10; browser steps 11–12 were skipped. The earlier Verify `39899325826` was cancelled at 04:28:10 UTC on SHA `dd59658274e93c65bd32c9f30ff0506c35aab982`; step 10 was cancelled and browser steps 11–12 skipped. Neither verified the profile screen.

## 2026-10-23 WBS 7.2.10 profile + password reset UI overlay (code SHA `30dd4e9eb016319d162d4f27bd8e2ca38164d351`, Verify pending)

The next independent member slice connects the pinned `PasswordChangePage` request chain: request a mail challenge, exchange the entered code for a reset token, then set the new password. New `mypageAdapter.ts` calls AIP `/auth/email/verification`, `/auth/password/verification`, and `/auth/password` through the existing `authTransport.fetch`, so Bearer authorization and 401 refresh are shared with the generated SDK client. `/auth/email/verify` is also exposed for the separate email-verification path. `apply-ui-overlay.sh` now copies `mypage.ts` and `mypageAdapter.ts`. Withdrawal remains deliberately unported and the overlay returns `PORT.NOT_PORTED`; no destructive transition is wired in this slice.

`node --test porting/facereview/ui-overlay/tests/*.test.ts` passed 25/25, including exact methods/paths, Authorization header, JSON payloads, reset-token forwarding, and error propagation. Browser-evidence unittest passed 15/15; `cargo fmt --all -- --check` and `git diff --check` passed. A local pinned UI build, browser flow, and live password-change integration were not run.

Verify `39900970945` at SHA `30dd4e9eb016319d162d4f27bd8e2ca38164d351` was cancelled at 04:36:33 UTC during isolation step 7; later core and browser steps were skipped. No CI browser journey verified its password overlay. The route inventory below separates source overlay implementation from browser proof.

## 2026-10-23 WBS 7.2.10 confirmed withdrawal overlay (code SHA `6b1a8d4f70688caa9a61ed32bba6095c86fae7f9`, Verify pending)

The pinned `/edit` flow already places withdrawal behind a separate confirmation action and logs out/clears local auth even when the logout request fails. The overlay now maps its `withdraw()` API to generated SDK `User.withdraw` targeting the signed-in member ID; the server owns child-row cleanup and session revocation. The adapter rejects when no member is signed in, and its idempotency key is generated by the existing transition helper. The overlay does not make an account deletion request before the UI calls the confirmed action.

TDD first failed with `api.withdraw is not a function`. After the implementation, `node --test porting/facereview/ui-overlay/tests/*.test.ts` passed 26/26. Browser evidence unittest passed 15/15, and `cargo fmt --all -- --check` plus `git diff --check` passed. Existing MySQL tests cover the server withdrawal transition, but no local UI browser or MySQL integration for this new binding was run. Verify `39901115868` for SHA `6b1a8d4f70688caa9a61ed32bba6095c86fae7f9` was in progress at 04:36:37 UTC during container initialization (step 2). Its current profile-focused browser journey does not exercise withdrawal.

### User UI route inventory — 25 unique paths

The pinned user UI has 30 HTTP call sites across six API modules, which collapse to 25 unique paths when methods sharing a path are grouped. The Socket.IO player is separate. “API/SDK” below means the server adapter, direct service test, or SDK journey; it does not mean every original REST request is reproduced as an AIP HTTP route. Browser proof is listed separately.

| User path (methods) | API/SDK evidence | Browser evidence at this snapshot |
|---|---|---|
| `/v2/auth/check-email` (POST) | AIP auth overlay and live auth service tests; not in the member SDK journey | Not run |
| `/v2/auth/login` (POST) | Live generated SDK journey, Verify `39851918809` | Login screen reached in `39885042974`; profile fixture login is in old pending run `39899325826` |
| `/v2/auth/signup` (POST) | Live auth service tests in the 7.2.8 auth slice | Not run |
| `/v2/auth/tutorial` (PATCH) | AIP `User.completeTutorial` service test; overlay function present | Not run |
| `/v2/auth/me` (GET) | Generated SDK member journey and `User`/`UserFavoriteGenre` reads | Verify `40010266672`: signed-in profile read rendered, browser/DB success |
| `/v2/mypage/profile` (PATCH) | AIP transition service test; new overlay uses one atomic generated-SDK `bundle()` | Verify `40010266672`: original form save, final visible render and MySQL persistence success |
| `/v2/auth/reissue` (POST) | One real refresh and retry in generated SDK journey `39851918809` | Verify `40010266672`: actual TTL edit-save, expired2/reissue1/retry2, browser/DB success |
| `/v2/auth/logout` (POST) | Generated SDK journey verifies logout and stale bearer rejection, `39851918809` | Verify `40017780097`: original MyPage logout button, one accepted logout response, cleared client auth, browser/DB success and revoked session +1 |
| `/v2/mypage/email/verification` (POST) | Backend mail challenge and stand-in provider tests, 7.2.8 slice B | Verify `40065518651` (`22089a4`): original MyPage verification banner/modal, one exact authenticated accepted POST per auth gate, badge/banner removal and reload persistence; browser/DB success, challenge0/mail1/flags GENERAL:0:1:1, unrelated 11 tables and other sessions preserved |
| `/v2/mypage/email/verify` (POST) | Backend verification-code tests, 7.2.8 slice B | Verify `40065518651` (`22089a4`): original MyPage code modal submit, one exact authenticated successful POST, badge visible/banner removed and reload verified; browser/DB success, target other columns and other account sessions preserved |
| `/v2/mypage/password/verification` (POST) | Backend password-reset challenge tests, 7.2.8 slice B | Verify `40062036395` (`0831456`): original password code exchange/reset, one exact accepted POST per gate; automatic logout body/cookie/client clear and new-password login/reload verified; browser/DB success, hash changed/old password refused/other device revoked/2 revoked·1 active/challenge0/11 tables preserved |
| `/v2/mypage/password` (PATCH) | Backend password reset, hashing, and session revocation tests, 7.2.8 slice B | Verify `40062036395` (`0831456`): original password code exchange/reset, one exact accepted POST per gate; automatic logout body/cookie/client clear and new-password login/reload verified; browser/DB success, hash changed/old password refused/other device revoked/2 revoked·1 active/challenge0/11 tables preserved |
| `/v2/mypage/withdraw` (DELETE) | Backend withdrawal and post-withdraw session tests, 7.2.8 slice A | Confirmed `/edit` action is wired to generated SDK `User.withdraw` at `6b1a8d4`; browser integration is outside the current CI journey |
| `/v2/home/video/recommend` (POST) | `VideoRequest.recommend` live service/oracle checks; overlay write adapter present | Form submission not browser-tested |
| `/v2/watch/comments` (GET, POST, PATCH, DELETE) | Comment reads/writes in service and generated SDK journey `39851918809` | Verify `40032647647` (`5741949`): original create/edit/delete, each one accepted mutation with exact fixture payload, reload render, browser/DB success; same ID, modified 0→1→1/deleted 0→0→1, soft-delete row and all unrelated comment rows preserved |
| `/v2/watch/like` (POST) | AIP service tests and overlay unit tests for add/cancel | Verify `40025235103`: original add/cancel clicks, one accepted mutation each, reload active true/false, browser/DB success, MySQL 0→1→0 and other members preserved |
| `/v2/home/category` (GET) | Live AIP extension and generated SDK checks | Actual React list response in `39885042974` |
| `/v2/home/search` (GET) | Live AIP extension and adapter tests | Verify `40038229142` (`f436a06`): original header found1/empty0, each submit/reload gate and PNG verified, four exact successful Video.homeSearch bodies, lifecycle22 complete; browser/DB success and all 11 business tables preserved |
| `/v2/home/video/all` (GET) | Live AIP extension and generated SDK journey | Actual React list response and fixture card in `39885042974` |
| `/v2/home/personalized` (GET) | Live AIP extension and generated SDK checks | Actual React list response in `39885042974` |
| `/v2/watch/video` (GET) | Live AIP extension/service tests | Watch fixture route was reached; external YouTube script was blocked and the UI showed its media error modal in `39896024733` |
| `/v2/mypage/videos/recent` (GET) | Service/oracle comparison for the mypage read extension | Not run in browser |
| `/v2/mypage/emotion/summary` (GET) | Service/oracle and write-extension tests for `User.myEmotionSummary` | Not run in browser |
| `/v2/watch/recommended` (GET) | Live AIP extension/service tests | Not run in browser |
| `/v2/home/bookmark` (GET, POST) | Generated SDK journey and AIP/MySQL tests | `39885042974` passed UI add/reload and remove/reload with DB 0→1→0; later `39896024733` failed its accessibility assertion under the media modal, so the later failure remains distinct |

All five account paths in this subset now have source overlay bindings: email challenge/verification, password reset, and confirmed withdrawal. The profile save/render is verified in `40010266672`, password in `40062036395`, and MyPage email verification in `40065518651`; withdrawal has existing server-side MySQL coverage but the new screen binding awaits browser integration. No full 25-path browser coverage is claimed.

### Administrator UI and API inventory — separate from the 25 user paths

The admin UI source has five screens (`users`, `videos`, `video-requests`, `comments`, plus the index route) and a fallback route. The admin API inventory has 13 feature paths; its four `/v2/auth/*` routes are shared auth paths and are not counted as additional user paths.

| Admin API path | API/backend evidence | Admin browser/overlay |
|---|---|---|
| `/v2/admin/users` (GET) | AdminService oracle and live MySQL checks; list slice matched 62/62 cases | Not ported or browser-tested |
| `/v2/admin/users/{user_id}/deactivate` (PATCH) | Role/ownership and soft-delete service tests | Source adapter `User.deactivate` (unit-tested only); not applied to the admin UI or browser-tested |
| `/v2/admin/users/{user_id}/role` (PATCH) | SUPER_ADMIN role-change service tests | Source adapter `User.changeRole` (unit-tested only); not applied to the admin UI or browser-tested |
| `/v2/admin/video-requests` (GET) | AdminService oracle and live MySQL checks | Not ported or browser-tested |
| `/v2/admin/video-requests/{request_id}/approve` (POST) | Durable job, MySQL, Mongo initialization and oracle checks | Not ported or browser-tested |
| `/v2/admin/video-requests/{request_id}/reject` (POST) | AIP transition and live service checks | Source adapter `VideoRequest.reject` (unit-tested only); not applied to the admin UI or browser-tested |
| `/v2/admin/videos` (GET) | AdminService oracle and live MySQL checks | Not ported or browser-tested |
| `/v2/admin/videos/{video_id}` (DELETE) | Soft-delete transition and service tests | Source adapter `Video.remove` (unit-tested only); not applied to the admin UI or browser-tested |
| `/v2/admin/comments` (GET) | AdminService oracle and live MySQL checks | Not ported or browser-tested |
| `/v2/admin/comments/{comment_id}` (DELETE) | Comment soft-delete transition and service tests | Source adapter `Comment.remove` (unit-tested only); not applied to the admin UI or browser-tested |
| `/v2/admin/system/status` (GET) | Redis status oracle and Linux host-metric checks; macOS CPU/memory/disk parity was not measured | Not ported or browser-tested |
| `/v2/admin/dashboard/business-stats` (GET) | Dashboard oracle matched 25 cases; live SQL/Mongo checks | Not ported or browser-tested |
| `/v2/admin/dashboard/signup-trend` (GET) | Dashboard oracle includes six period cases; live SQL checks | Not ported or browser-tested |

Admin backend work is therefore separate from the unported admin React screens. The admin API/service cases are not evidence of admin SDK wiring, route-state behavior, screenshots, or zero REST fallback.


## 2026-10-23 Verify 상태 정정, 로컬 PostgreSQL 실패 원인, 관리자 쓰기 adapter 조각

**Verify `39901115868` (SHA `6b1a8d4f70688caa9a61ed32bba6095c86fae7f9`) 판정: cancelled, 증거 없음.** 2026-10-23 04:50:45 UTC에 완료된 job은 `core`이고, step 10(`Verify compiler, runtime, product, worker, transport and SDK`)이 04:50:37에 cancelled, step 11(pinned React 준비)과 12(HTTP/worker/MySQL/browser)는 skipped였다. 원인은 `verify.yml`의 `concurrency: group: verify-${{ github.ref }}`, `cancel-in-progress: true`이다. 다음 push인 `93890fa`가 04:50에 같은 ref로 새 Verify를 시작하면서 이 run을 취소했다(취소 시점이 새 run `39902365530`의 시작 04:50:49와 맞는다는 점에서 추론; 취소 이벤트의 actor는 조회하지 않았다). step 13의 artifact upload는 success로 표시되지만 browser step이 실행되지 않았으므로 회수할 screenshot은 없다고 본다(artifact 목록은 조회하지 않았다: 확인 필요). 따라서 profile write, password reset, 확인 탈퇴 binding의 browser 증거는 여전히 없다. 같은 이유로 `d29f1d3`·`30dd4e9` run도 취소됐다. **연속 push는 어떤 Verify도 browser step까지 가지 못하게 한다.** 브라우저 증거가 필요한 SHA는 Verify가 끝날 때까지 다음 push를 보류해야 한다.

**Verify `39902365530` (SHA `93890fa011c3a69e03d30e2a665ceb99c49d12e4`).** 2026-10-23 04:50:49 UTC 시작, 04:52 UTC 조회 때 `in_progress`, step 2(`Initialize containers`)였다. 04:55 UTC 재조회에서도 `in_progress`. 결과는 아직 없다.

**로컬 Rust 전체 lib 검사의 PostgreSQL 21건 실패는 시험 DB를 띄우지 않은 탓이다.** 이전 Run의 일회성 DSN `127.0.0.1:54321`에는 listener가 없었다(`lsof -nP -iTCP:54321 -iTCP:5433 -sTCP:LISTEN` 결과 없음). 이번 Run이 소유한 `postgres:17` 컨테이너(`aip-wbs-run-pg`, `POSTGRES_USER=runner`, `POSTGRES_DB=aip_test_ci`, trust 인증, `127.0.0.1:54321`)를 띄우고 같은 DSN으로 실행했다.

| 명령 | 결과 |
|---|---|
| `env -u PGHOST -u PGPORT … AIP_TEST_DATABASE_URL='host=127.0.0.1 port=54321 dbname=aip_test_ci user=runner' AIP_TEST_ADMIN_URL='host=127.0.0.1 port=54321 dbname=postgres user=runner' cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --lib --locked --offline` (첫 실행, 컨테이너 시작 직후) | `95 passed; 3 failed`. 실패는 `admission::tests::browser_websocket_is_limited_after_its_auth_frame`, `pool::tests::canceled_query_lease_is_discarded`, `pool::tests::concurrent_checkouts_use_distinct_bounded_connections`이고 모두 `Err(Timeout)` |
| 같은 3개를 `--test-threads=1`로 재실행 | `3 passed; 0 failed` |
| 같은 전체 lib 재실행 | `98 passed; 0 failed` |

`Connection refused` 21건이 사라졌으므로 원인은 DB 부재였다. 첫 실행의 Timeout 3건은 같은 코드를 다시 돌리면 통과하는 일시 실패다. 컨테이너 초기화 직후의 첫 연결 지연과 병렬 실행의 경합이라는 설명은 추론이며 계측하지 않았다. 이 PostgreSQL 단위 시험은 MySQL/Mongo/Redis 종단(`mysql_facereview_watch`)을 대체하지 않는다. 컨테이너는 `docker rm -f aip-wbs-run-pg`로, 이 Run이 만든 `spikes/spike-v6-transport/target`(1.3G)은 `cargo clean`으로 회수했다. 이름이 `aip-p730-*`, `aip-fixture-*-p730`인 기존 컨테이너는 소유가 불명확해 건드리지 않았다.

**관리자 쓰기 adapter 조각(원본 `new-facereview-admin-front` pinned `ab0386c878a5ba5c7c079a262b987c10d211d22d`).** `porting/facereview/ui-overlay/src/api/adminAdapter.ts`는 관리자 화면의 쓰기 5개를 선언된 AIP transition에 연결한다: `deactivateUser`→`User.deactivate`, `changeUserRole`→`User.changeRole`(`role`만 patch), `deleteVideo`→`Video.remove`, `deleteComment`→`Comment.remove`, `rejectVideoRequest`→`VideoRequest.reject`(`admin_comment`만 patch). 모든 호출은 `apply()`와 호출별 idempotency key를 쓰고, 성공은 원본 axios 응답 모양 `{ data: { message }, status: 200 }`으로 돌려준다. 서버 거절은 서버 code를 가진 오류로 전달된다. 권한 판정(`isAdmin(actor)`, SUPER_ADMIN 전용 역할 변경, ADMIN은 GENERAL만 비활성화)은 정의가 맡고 adapter는 확인하지 않는다. 알 수 없는 role, 빈 대상 id, 빈 거절 사유는 요청 전에 `CLIENT.BAD_ARGUMENT`로 거절한다. 성공 메시지 문구는 원본 서버의 문자열을 대조하지 않은 UI용 문구다.

TDD: 구현 전 `node --test porting/facereview/ui-overlay/tests/adminAdapter.test.ts`는 `ERR_MODULE_NOT_FOUND`(adminAdapter.ts)로 실패했고, 구현 뒤 `tests 4 / pass 4 / fail 0`이다. 범위 한계: 이 adapter를 admin UI checkout에 복사하는 스크립트와 `api/admin.ts` 치환, 읽기 8개(목록 4, 대시보드 2, 시스템 상태 1)·영상 요청 승인(durable job) adapter는 없다. admin UI의 build, 화면, 브라우저는 실행하지 않았다.

**Verify `39902365530` (SHA `93890fa`) 판정: failure, 원인 확인.** step 12의 password login 시험 `facereview_accounts_log_in_refresh_and_log_out_without_the_originals_replay_defect`가 `mysql_facereview_auth.rs:596`에서 panic했다. 직전 로그에 member SDK journey 줄이 있으므로 실패 지점은 profile React journey 호출이다. `facereview-ui-profile-browser.py` line 20의 `from product.tests...`가 `ModuleNotFoundError: No module named 'product'`였다. 이 스크립트는 repo root를 `sys.path`에 넣지 않았고(member 스크립트는 넣는다) 이 run이 처음 실행된 것이다. profile/reset/withdraw의 브라우저 증거는 아직 없다. step 11까지와 `http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction`은 통과했다. 수정은 `sys.path.insert`를 member 스크립트와 맞춘 두 줄이며 로컬에서는 import만 확인했다(playwright 미설치로 브라우저 미실행). `mysql_facereview_watch`의 빈 timeline 단정은 이 run에서 확인하지 못했다.

## 2026-10-23 Verify 39905029803 결과와 player fixture 보정

Verify `39905029803`(SHA `654eb4c3d9f6e5cdbd935863d84509247d59ed72`)는 step 12 `Verify real HTTP to Node and Python workers to MySQL`에서 실패했다. profile 단계는 실패하지 않았다. 실제 pinned React UI에서 표본회원 이름과 선택 장르(`eating`, `horror`)를 읽었고, 변경 뒤 이름(`AIP 프로필 수정회원`)과 장르(`animal`, `eating`)가 browser JSON에 기록됐다. `write_bundle_ok=true`, API `/read`·`/apply` contract fingerprint 일치, 뒤이은 MySQL profile 행 assertion 통과로 이 프로필 쓰기 slice는 Linux CI에서 확인했다. `profile-before-save.png`와 `profile-after-save.png` screenshot도 열어 확인했다.

전체 step은 그 뒤 기존 member bookmark journey를 실행하다 reload 뒤 실패했다. 원시 step output과 safe artifact에서 `journey_stage=bookmark_reloaded`, `route_class=watch_fixture`, watch content visible, unhandled page errors 0을 확인했다. DOM allowlist에는 `즐겨찾기 해제` `aria-pressed=true`가 있었으나 accessibility locator는 보이지 않았다. 외부 YouTube script 두 건은 의도한 차단에 따라 `net::ERR_FAILED`였고 UI의 video-load error dialog가 떠 있었다. 앞선 UI 확인에 따르면 그 CTA는 `/`로 이동하므로, role selector를 느슨하게 하거나 CTA를 눌러 가리지 않는다. 이는 bookmark 상태 값 불일치가 확인된 실패가 아니다. 이후 SQL bookmark 단정은 미검증이다.

수정은 브라우저 fixture가 실제 YouTube·영상 provider 요청을 모두 차단하되 `youtube.com/iframe_api` script 요청에만 로컬 no-op player API를 돌려주게 한다. 이 테스트는 bookmark UI를 확인하기 위한 것이며 영상 로드·재생 또는 ML 추론 증거가 아니다. 추가된 helper 단위 시험은 정확한 host/path/resource type만 fixture 처리하는지 확인한다. 로컬 `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence`는 16/16, 두 browser Python 파일 `py_compile`과 `git diff --check`도 통과했다. 실제 브라우저에서의 보정 효과는 다음 Linux Verify가 끝나기 전까지 미검증이다.

`gh run view 39905029803 --log-failed`는 Codex Mac nested worker 실행에서 log-cache 쓰기(`/Users/winterholic/.cache/gh/run-log-39905029803-1792732710.zip`)가 `operation not permitted`로 막혔다. `gh api .../actions/runs/39905029803/logs`로 내려받은 원문 step 12를 대신 검사했다. 실행 artifact `12480314914`(206,614 bytes)는 profile/add safe JSON과 screenshots를 확인한 뒤 삭제했고, artifact 목록 재조회는 0개였다.

profile success는 계정 화면 write 한 조각만 다룬다. 25개 사용자 경로 전체, 관리자 UI, browser bookmark on/off와 SQL 0→1→0, 전체 prerender는 미완료다. model.h5 추론과 visitor policy는 계속 보류한다.

## 2026-10-23 Verify 39907973571 결과와 `/read` 응답 형태 진단

Verify `39907973571`(SHA `cce3987b2197463f1eb1bbf7861e17d3a6aa3c20`)는 step 12에서 실패했다. profile browser journey 및 MySQL profile 행 assertion은 다시 통과했다. before screenshot은 profile edit 화면을 보였지만 after screenshot은 로고만 있고 본문이 비어 있어 결과 화면 증거로 인정하지 않는다. 후속 bookmark browser journey는 `journey_stage=completed`였고 `add-bookmarked.png`에서 `저장됨` UI를 확인했다. 다만 후속 strict `/read` gate가 실패했으므로 Rust caller의 뒤이은 bookmark SQL assertion은 실행되지 않았다.

실패 원문은 `gh api repos/winterholic/Application-Intent-Protocol/actions/runs/39907973571/logs`로 조회했다. `mysql_facereview_auth.rs:611`은 browser script의 `failed_read_requests()` 실패를 받아 panic했다. safe evidence에 든 `VideoBookmark` POST `/read`는 HTTP 200, contract fingerprint 일치, query shape 허용이며 boolean `ok`가 없었다. raw server log에는 설명할 `WIRE.*` code가 없었다. 이 artifact에는 response body shape가 없으므로 API 결과가 왜 boolean `ok`를 포함하지 않았는지는 미확정이다. 이전 `gh run view 39907973571 --log-failed`는 gh cache 경로에 zip을 쓰다 `operation not permitted`가 났으므로 같은 명령을 반복하지 않고 API logs endpoint를 사용했다.

run artifact `12482507650`(191,611 bytes)의 profile/add safe JSON과 screenshot을 확인한 뒤 삭제했고, artifact 재조회는 0개다. 다음 진단 수정은 response 원문/값을 저장하지 않고 object/list/null 형태, allowlist 내 key 이름, 미지 key 수, `ok` 자료형, `rows` 길이, allowlist된 wire code만 기록한다. 동시에 YouTube iframe API script에는 `YT` 객체나 callback을 만들지 않는 inert JavaScript를 반환해 외부 provider 요청을 유지 차단한다. 이는 실제 media playback 검증이 아니다. 이번 변경 결과는 후속 Verify 완료 전까지 미검증이다.

## 2026-10-23 Verify 39910753805 결과와 수정 반복 상한

Verify `39910753805`(SHA `bd3ef4abc6bdc678f939134e78fd4bea08e94f80`)는 세 번째 대기 시도 중 완료됐고 `failure`다. step 12 `Verify real HTTP to Node and Python workers to MySQL`의 `facereview_accounts_log_in_refresh_and_log_out_without_the_originals_replay_defect`가 `crates/aip-cli/tests/mysql_facereview_auth.rs:611`에서 browser script의 nonzero exit 때문에 실패했다. `gh run view 39910753805 --log-failed`는 gh log-cache 경로 쓰기 권한 오류가 이미 확인돼 재시도하지 않았다. 대신 `gh api repos/winterholic/Application-Intent-Protocol/actions/runs/39910753805/logs`의 raw step 12를 읽었다.

이 run은 앞선 `/read` gate에서 boolean `ok`가 없던 문제를 안전하게 분류하기 위한 두 번째 수정 후 실행됐다. add-bookmark 단계의 `VideoBookmark` POST `/read`는 HTTP 200, runtime contract fingerprint 일치, 허용된 query shape를 가졌지만 `response.json()`이 실패해 browser script는 `response_shape: {"json_kind":"invalid_json"}`을 남겼다. raw server log에는 route `read`, status 200만 있고 관련 `WIRE.*` code가 없다. 응답 본문과 비공개 값을 evidence로 보존하지 않도록 설계했으므로 실제 body와 서버 측 생성 경로는 확인되지 않았다. 이 때문에 valid API success나 구체적 서버 root cause를 주장할 수 없다.

artifact `12483616670`(191,662 bytes)의 add/profile JSON과 screenshot을 확인한 뒤 삭제했고 artifact 재조회는 `total_count=0`이다. Add JSON은 `journey_stage=completed`, page error 0이며 screenshot에 `저장됨`이 보인다. 그러나 failed read gate 뒤 Rust caller의 MySQL bookmark count assertion은 실행되지 않았다. Profile fields와 저장 row assertion은 통과했다. profile after screenshot은 상단 logo만 보여 최종 화면 evidence로 세지 않는다.

두 번째 수정 시도가 실패했으므로 지시된 수정 반복 상한에 도달했다. 추가 코드 수정은 하지 않고, Verify green이 선행 조건인 관리자 UI slice도 시작하지 않는다. 이후에는 이 safe evidence가 지목한 response 생성 경로를 원인 추정 없이 조사하고, 새로운 push가 필요하면 현재 run이 종료된 상태임을 확인한 뒤 진행한다. model.h5 inference와 visitor policy는 계속 보류한다.

## 2026-10-23 browser `/read` 측정기 조사와 계측 (SHA `5a42b20`, Verify `39915678255`)

`39910753805`의 `invalid_json`은 서버 non-JSON의 증거가 아니었다. 수집기가 `response` 이벤트(본문 완료 이전) 안에서 `response.json()`을 부르고 모든 Exception을 같은 라벨로 기록했기 때문이다. 서버 로그 대조에서도 그 run의 add 구간은 `apply`(Video.view), `read`(VideoBookmark), `apply`(북마크 생성) 순인데 Playwright row에는 마지막 `/apply`가 없었다. 같은 Playwright 1.64/Chromium 1248에서 합성 서버로 재현한 실패 유형은 `resource_evicted`, `TargetClosedError`, 요청이 `response`는 있으나 `requestfinished`/`requestfailed` 없이 끝나는 경우다.

계측 `5a42b20`: 본문은 `requestfinished`에서 `response.body()`로 읽고 메모리에서 parse한다. 보존 값은 content-type의 media type, 바이트 길이, sha256 앞 16자, allowlist shape, 실패 reason code(`target_closed`, `resource_evicted`, `timeout`, `playwright_error`, `request_failed_after_response`, `not_finished_before_report`)뿐이다. 예외 원문, 토큰, 쿠키, 회원 값은 저장하지 않는다. 요청별 correlation id, request/response/finished/failed/body 시각, reload/close/navigation timeline, 이벤트가 reload 전후 어느 구간인지(`timeline_phase`)를 기록한다. 앱 쪽은 init script가 fetch를 감싸 VideoBookmark `/read`에 한해 앱이 받은 길이·해시·parse 결과를 console로 내보내며 원 응답은 그대로 통과시킨다. Playwright 캡처와 앱 수신의 길이·해시가 같을 때만 `match`이고 본문 미취득, 앱 기록 누락은 별도 값이다. 종료 이벤트가 없는 VideoBookmark `/read`와 본문 미취득은 성공으로 세지 않고 "measurement gap"으로 실패한다.

로컬 검증: 단위 22/22. 가짜 UI(별도 포트, 5173의 다른 프로젝트 서버는 건드리지 않음)와 가짜 CORS API로 `main()` 전체를 실제 Chromium에서 실행했다: 정상 add/remove(`match`, unfinished 0), 서버 HTML 응답(`json_decode_error`, `first_byte_class=html`, 앱도 같은 해시), 낙관적 UI와 reload 겹침(200 row, `not_finished_before_report`, 앱 수신 없음)이 서로 다른 값으로 기록됐다.

CI: Verify `39915678255` attempt 1은 step 10의 `spike-v3-write/tests/absence_serializable.rs:78` 동시성 시험 실패(`CONFLICT`가 기대한 `FORBIDDEN` 자리)로 browser 단계 전에 끝났다. 같은 SHA의 실패 job 재실행(attempt 2)에서 step 12까지 갔고 add가 실패했다. 증거: lifecycle 15요청 전부 종료, VideoBookmark `/read` 두 건 모두 JSON object, 앱 수신과 `match`, page error 0. 실패는 클릭 직후 `aria-pressed=true` 단정이며 UI는 미눌림, 제목과 조회수가 빈 화면이다. 서버는 북마크 생성 `/apply`를 200으로 처리했다. 핀된 UI(`WatchPage.tsx`, overlay `readAdapter.getVideoDetail`)상 videoDetail은 `Video.view` apply 뒤에 resolve되고 `onMutate`가 진행 중인 조회를 취소하며 낙관적 갱신은 기존 데이터가 있을 때만 일어난다. 따라서 클릭이 videoDetail 완료보다 앞서면 flip이 없다(추론). 반대로 flip이 있으면 스크립트가 곧바로 reload해 진행 중 요청을 끊는다. 두 증상은 한 원인(완료 신호가 아닌 낙관적 `aria-pressed`와 즉시 reload)의 서로 다른 타이밍이다. `39910753805`의 body 실패 사유 자체는 당시 계측이 없어 미확정이다.

profile: `profile-browser-result.json`의 MySQL 이름·장르 assertion은 통과(`saved_name`, `saved_genres`, `write_bundle_ok`)지만 `profile-after-save.png`는 헤더만 있는 빈 본문이다. 두 run 연속이라 화면 결과 증거가 아니며, 이동 직후 screenshot과 handler 안 `response.json()`이라는 같은 패턴이 원인 후보다(미확인).

최소 수정안(미적용): (1) 클릭 전 `.video-information-container .title`에 fixture 제목이 보일 때까지 대기. (2) 클릭 후 reload 전에 북마크 `/read`와 생성/해제 `/apply`가 종료 이벤트에 도달할 때까지 대기. (3) 그 뒤 reload와 `aria-pressed` 단정. 단정 완화, API mock, 숨은 버튼 조작은 쓰지 않는다. 제품 코드 변경은 필요하지 않다. 원시 증거: artifact `12486124697`(삭제하지 않음), outbox `ci-39915678255-attempt1/`·`attempt2/`, 재현 도구 `aip-measurement-repro/`.

## 2026-10-23 browser journey 수정과 Verify 39922363131 (SHA `d6718c4`)

수정(member·profile 한 커밋): member는 클릭 전 `.video-information-container .title`의 fixture 제목을 기다리고, 클릭 뒤 북마크 `/read`와 `/apply`가 `requestfinished`(본문 수집 포함)에 이를 때까지 기다린 다음 reload와 `aria-pressed` 단정을 한다. profile은 본문을 `requestfinished`에서 읽고 저장 `/apply` 본문과 목적지 페이지의 읽기, `networkidle`을 기다려 screenshot을 찍으며 `after_save_visible_text_length`를 기록한다. 본문을 못 읽은 행은 성공으로 세지 않고 "measurement gap"으로 실패한다. 제품 코드, 단정 완화, mock, 숨은 버튼 조작은 없다.

로컬: 단위 22/22. 가짜 UI/API에서 수정 전 스크립트는 느린 `Video.view`에서 `bookmark_action` 미눌림, 낙관적 flip 뒤 느린 `/read`에서 `bookmark_reloaded` 실패(unfinished 북마크 `/read`)였고 수정본은 add/remove/profile 모두 통과했다. 가짜 UI의 `Video.view` 뒤 제목 렌더, 클릭 전 데이터가 없을 때 낙관적 flip 없음은 핀된 UI 코드 읽기에 근거한 모사다.

CI: Verify `39922363131` attempt 1은 step 12의 `mysql_worker_write_e2e.rs:433`에서 `retryJob`이 `WIRE.DB_UNAVAILABLE`(retry later)를 받아 실패했고 브라우저 시험은 실행되지 않았다. 같은 SHA의 실패 job 재실행(attempt 2)에서 `success`. 증거: add/remove `journey_stage=completed`, 북마크 `/read` 앱 수신과 `match`, `bookmark_requests_settled`가 `reload_start`보다 앞, add 화면에 제목·`저장됨`·조회수 11, profile 이름·장르 assertion 통과와 body 전부 수집. profile after 화면은 DOM 텍스트 133자인데도 헤더만 보이는 빈 화면이라 증거로 세지 않는다(렌더링 지연은 미확인 후보). 간헐 실패 후보 두 건(`absence_serializable.rs:78`, `mysql_worker_write_e2e.rs:433`)은 STATUS에 별도 기록했다.

## 2026-10-23 WBS 7.2.9 / 7.2.10 관리자 콘솔 실제 SDK 전환 (원본 `new-facereview-admin-front` 고정 `ab0386c878a5ba5c7c079a262b987c10d211d22d`)

**원본 위치.** 이전 보고의 "checkout 없음"은 사실이 아니었다. 로컬 `~/development/projects/facereview/new-facereview-admin-front`가 있고 HEAD가 고정 commit과 같다(작업 트리의 변경은 scss/svg 6개이며 `git diff --ignore-cr-at-eol --stat`로 줄바꿈 차이임을 확인). 원본 checkout은 고치지 않았고, `git clone --local --no-checkout`으로 만든 임시 clone에서 고정 commit을 체크아웃해 overlay를 적용했다. remote `https://github.com/winterholic/new-facereview-admin-front.git`는 public이며 `git ls-remote`가 같은 commit을 돌려준다(CI 준비 단계가 이 URL을 쓴다).

**구현.** (1) `ui-overlay/src/api/adminReadAdapter.ts`: 읽기 7개(회원·영상 요청·영상·댓글 목록, 시스템 상태, 비즈니스 지표, 가입 추이)와 `/auth/me`를 원본 응답 모양으로 조립한다. 조립 규칙은 `reference/slice13e_oracle.py`·`slice14h_oracle.py`가 원본 `AdminService`와 대조한 것(목록 62건, 대시보드 25건 일치)을 옮겼다. 이전 문서의 "읽기 8개"는 목록 4, 대시보드 2, 시스템 상태 1로 7개를 적고 있어 8번째를 `/auth/me`로 해석했다(해석, 확인 필요). (2) `adminAdapter.ts`의 `createAdminApprovalApi`: 승인은 durable job `approveVideoRequest`를 enqueue하고 `SUCCEEDED`까지 기다린 뒤 결과의 `video_id`를 돌려준다. `FAILED`, `CANCELLED`, 시간 초과는 오류이며 성공으로 보고하지 않는다. 이 job은 외부 YouTube 호출이 없다(정의의 `access`에 `http.*` 없음). (3) `ui-overlay/admin/src/api/{aipClient,admin,auth,HeaderToken}.ts`와 `porting/facereview/apply-admin-ui-overlay.sh`. 기존 쓰기 5개 adapter는 그대로 두고 `writeAdapter` 타입 의존만 자체 인터페이스로 바꿨다.

**SDK 결함 2건(실제 콘솔을 돌린 뒤에야 드러남, 가짜 client 시험은 통과하던 것).** ① 생성 TS SDK에 독립 aggregate(`expose aggregate`: 총계, 가입 수, 주간 활성, 장르 조회수) 호출이 없었고 `typed.ts`의 읽기 검증이 `read`와 `select`가 없는 질의를 거절했다. `aggregate()`를 추가했다. 첫 구현은 `filter: []`를 항상 보내 `callerFilter none` 집계가 `WIRE.FILTER_NOT_ALLOWED`로 거절됐고, 실제 브라우저 실행에서 발견해 호출자가 줄 때만 보내도록 고쳤다. ② `jobs.ts`의 스칼라 검증이 `Uuid`를 몰라 UUID 키 job(`approveVideoRequest`의 `request`, `video_id`)을 enqueue할 수 없었다(`WIRE.BAD_VALUE`, 서버 전송 전). `typed.ts`와 같은 규칙을 넣었다. 각각 수정 전 실패(`client.aggregate is not a function`, jobs 시험 1건 실패)를 확인한 뒤 고쳤다.

**쓰기 5개와 승인의 원본 대조(필드, 권한, 상태).**

| 동작 | 원본(`AdminService`, `admin.py`) | AIP 선언 | 브라우저+DB 확인 |
|---|---|---|---|
| 회원 비활성화 | `admin_required`; SUPER_ADMIN은 누구나, ADMIN은 GENERAL만; `is_deleted=1`과 그 회원의 모든 댓글 `is_deleted=1` | `User.deactivate`: 같은 허용 조건, 댓글 `update many` (maxRows 1000) | 데이브: `GENERAL`/삭제 1, 그의 댓글 삭제 1, 미삭제 댓글 0 |
| 권한 변경 | `super_admin_required`; 대상 `is_deleted=0`; `role` 대입 | `User.changeRole`: SUPER_ADMIN만, `from is_deleted=false`, `role`만 patch | 에린 `ADMIN 해제` 후 `GENERAL`/삭제 0 (ADMIN 지정 방향은 브라우저에서 미실행) |
| 영상 삭제 | `admin_required`; `is_deleted=1` | `Video.remove`: `isAdmin`, soft delete | 영상 행 `is_deleted=1`, 목록 3건에서 2건 |
| 댓글 삭제 | `admin_required`; `is_deleted=1` | `Comment.remove`: `isAdmin`, soft delete | 댓글 `is_deleted=1`, 행은 남고 총 5건 유지 |
| 요청 거절 | `admin_required`; PENDING만; `REJECTED`와 `admin_comment` | `VideoRequest.reject`: `isAdmin`, `from status = PENDING`, `admin_comment` patch | `REJECTED`와 `중복 영상입니다` |
| 요청 승인 | 한 트랜잭션에서 영상 생성, Mongo 초기 문서 2개, 요청 `ACCEPTED` | durable job: `Video` create, Mongo upsert 2개, `VideoRequest.accept` | job `SUCCEEDED`, 요청 `ACCEPTED`, 영상 `승인 표본 영상`/`표본 채널`/321/vlog/조회 0/활성, Mongo 문서 각 1개 |

차이: 원본 오류는 한국어 메시지, AIP는 `WIRE.*` code이며 콘솔은 둘 다 일반 토스트로만 보여 준다. 성공 메시지 문구는 원본 서버 문자열과 대조하지 않았다. 삭제된 영상을 다시 삭제하는 경우처럼 원본이 `VIDEO_NOT_FOUND`로 답하는 경계는 이번 브라우저 여정에 없다.

**새 실서비스 시험 `crates/aip-cli/tests/mysql_facereview_admin_ui.rs`.** 비밀번호 로그인 배포에 쓰기 worker와 Mongo 쓰기 컬렉션을 켠 서비스, 실제 MySQL 8.4, Mongo, Redis. 브라우저 전에 서버 정책을 정확한 code로 확인한다. 관리자는 6개 집계와 3개 확장에 모두 답을 받는다(대조군). 일반 회원과 비인증 호출자는 6개 집계에서 `WIRE.ACCESS_DENIED`, 확장에서 각각 `WIRE.EXTENSION_ERROR`, `WIRE.UNAUTHENTICATED`, 승인 job에서 `WIRE.ACCESS_DENIED`, `WIRE.UNAUTHENTICATED`를 받고, 일반 회원의 자기 역할 상승은 `WIRE.FORBIDDEN`이다. 첫 탐침은 요청 형식이 틀려 엉뚱한 code로 거절돼 의미가 없었고, 대조군을 넣어 고쳤다. 브라우저 여정은 `product/tests/facereview-ui-admin-browser.py`(기존 member, profile 여정과 별도 스크립트, 별도 증거 파일)다.

**로컬 실행 결과(macOS, 이 Mac의 run 소유 컨테이너 MySQL 8.4, mongo:7, redis:7, 고정 commit에 overlay를 적용한 임시 clone, Vite 5183).**

| 명령 | 결과 |
|---|---|
| `node --test porting/facereview/ui-overlay/tests/*.test.ts` | adminReadAdapter 12/12, adminAdapter 9/9, 전체 47/47 |
| `node --test` 관련 20개 파일(overlay 6개, SDK client 14개 등) | `tests 147 / pass 147 / fail 0` |
| `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` | 25 tests OK(새 헬퍼 3개 포함) |
| 고정 admin UI: `apply-admin-ui-overlay.sh`, SDK tarball 설치, `npx tsc -b`, `vite build` | `tsc-exit=0`, build 성공, 번들에 `/v2/` 문자열 0건 (적용 전 원본 단독 build도 성공했다) |
| `cargo test -p aip-cli --features mysql --test mysql_facereview_admin_ui -- --ignored` | 서버 정책만: `1 passed`; 브라우저 포함 전체: `test result: ok. 1 passed`, 약 20초 |
| `cargo clippy -p aip-migrate -p aip-service -p aip-cli --features mysql,aip-cli/mysql --all-targets --locked -- -D warnings`, `cargo fmt --all --check` | 통과 |

브라우저 여정이 확인한 것: 비인증 방문은 `/login`으로 이동하고 관리자 요청이 없다. 일반 회원은 로그인에는 성공하지만 콘솔이 `관리자 권한이 없습니다.`로 거절하고 세션을 남기지 않으며 집계, 확장, job 요청이 0건이다. SUPER_ADMIN 로그인 후 회원 목록(전체 6건, 키워드 검색 1건), 쓰기 5개와 승인 job, 영상 3건에서 2건, 댓글 5건, 로그아웃 후 세션 제거. 모든 SDK 요청이 `ok:true`이고 계약 지문이 서버와 같으며 원본 REST 호출은 0건이다. 위 DB와 Mongo 단언 통과.

**화면 coverage와 미검증.** 5개 화면 중 4개(회원, 영상, 영상 요청, 댓글)와 로그인을 로컬 브라우저에서 확인했다. **대시보드는 화면으로 확인하지 못했다.** 이 macOS에서 `User.systemStatus`가 호스트 수치를 null로 주고(서버 문서화된 Linux 전용), 고정 콘솔의 대시보드는 `cpu_usage.toFixed`를 호출하므로 로그인 직후 React 오류(`TypeError`)가 난다(원인은 코드 읽기와 null 관측에서 추정, 오류 메시지는 확인 못함). 같은 실행에서 대시보드의 SDK 요청(`signupCount` 7회, `weeklyActive`, 장르별 `totalCount`와 `categoryViews`, `contentHealth`, `pipeline`, `systemStatus`)은 모두 `ok:true`였으므로 API 연결은 확인했지만 렌더는 Linux CI 시험(대시보드 제목, KPI, CPU 줄, 연결 칩 `정상` 3개)에 맡긴다. 기간 선택(7d 외), 토큰 만료 reissue, `ADMIN 지정` 방향, ADMIN 계정이 GENERAL만 비활성화하는 경계의 브라우저 시험은 없다(서버 정책 시험은 기존 `mysql_facereview_auth`에 있음). 이 로컬 결과는 macOS Seatbelt worker 기준이며 Linux Bubblewrap CI 결과가 아니다. CI 결과는 이 SHA의 Verify 확정 후 별도로 기록한다. 사용자 UI 25 고유 경로와 관리자 API 13 경로의 분모는 합치지 않았다.

**이전 dirty clone 보존 patch.** Run `a14a5dd2-8b40-4471-9bca-967ab33d9ee0`의 `aip-ui-dirty-clone-preservation/`는 읽지 못했다. `ls`가 ttakkari 가드에 거부됐고(우회하지 않음), `list_artifacts`(agent 영역, 이름 검색)는 빈 목록이었다. 미확인이다. 이번 Run은 사용자 소유 자료나 dirty 사본을 삭제하지 않았다.

**CI 결과 (같은 SHA, 이 절의 로컬 결과와 구분).** 코드 SHA `e4f2efe97a7f712c3c8438abfdb8f5eb4e43a4d3`(origin/main 일치)의 Verify `39932769334`는 `completed success`다. step 10(core: 새 overlay 시험 6개 파일과 `aggregate.test.ts`를 `tools/verify.sh`에 넣은 상태), 11(member UI 준비), 12(고정 admin UI 클론, overlay 적용, `npm ci`, SDK tarball 설치, `tsc -b`, `vite build`), 13(실제 HTTP, worker, MySQL, 브라우저 여정, 새 `mysql_facereview_admin_ui` 포함), 14 모두 `success`. 이 push 전에 추가 코드 push는 없었다(취소 반복 없음). artifact `facereview-ui-browser-39932769334-1`의 `admin-browser-result.json`: `journey_stage=logout`, `failure_type` 없음, 페이지 오류 0, API 요청 82개 모두 `ok:true`, 계약 지문 일치, 원본 REST 호출 0건. **`dashboard: rendered`** (Linux): `admin-dashboard.png`에 연결 칩 MySQL, Redis, MongoDB `정상`, CPU 53%·메모리 21%·디스크 59%, 신규 가입 추이, 장르별 조회수(animal, cook)가 보인다. 로컬 macOS에서 못 본 대시보드 렌더는 이 CI가 닫았다. 같은 run의 member add/remove/profile 여정 증거는 이번 변경 대상이 아니며, profile after-save 화면의 헤더만 보이는 증거 갭(`after_save_visible_text_length=133`)은 별도 추적 대상으로 그대로 남아 있다(원인 단정 없음). 로컬(macOS Seatbelt)과 CI(Linux Bubblewrap)는 별개 실행이다. 원시 증거는 이번 Run outbox의 `admin-browser-evidence-local/`(로컬)과 `ci-39932769334-artifact/`(CI)에 있고 GitHub artifact는 삭제하지 않았다.

## 2026-10-23 관리자 권한경계, null metrics 및 원본 API 분모 정정 (코드 SHA `1e528aa4a2c47ca02a453c1c4f1f7c0075e5380f`)

이 후속은 위 `e4f2efe` Verify green의 기존 범위를 유지하면서 다음 admin slice를 추가했다. **최종 SHA의 Verify는 green이 아니다.** `39939106930` public Actions 요약은 Failure, core job annotation은 `Process completed with exit code 101`이다. `gh run watch`의 check-log 요청은 `403 Forbidden (checks:read 누락)`, 다음 `gh run view`는 API rate limit 403이었다. public UI에서 job 로그는 로그인 필요였고 artifact row에서 파일 내용을 열지 못했다. step 10–12 success, step 13은 직전 관측 때 in progress였지만 이후 실패한 정확한 step과 실패 assertion은 확인하지 못했다. artifact `facereview-ui-browser-39939106930-1` (560 KB, digest `sha256:7f09e2bdc026ce4b8848799b9336de5558c1b1de2a8bda609b03ad0fa7fa72ff`)의 내용, 이번 run의 browser request/page 결과도 미확인이다. exit 101을 특정 테스트의 실패 원인으로 해석하지 않는다.

### 원본 분모 전수 대조

고정 관리자 원본 `src/api/admin.ts`의 exports와 실제 화면 호출처를 SDK binding 및 overlay 사용처와 대조했다. 관리자 feature API는 **13개: 읽기 7 + 쓰기 6**이다. 읽기는 사용자·요청·영상·댓글 목록 4개와 system status·business stats·signup trend 3개다. 쓰기는 관리자 기존 5개와 승인 1개다. `/auth/me`는 `src/api/auth.ts`의 shared auth 4개 중 하나이고 login 화면의 사용자 확인에 쓰이므로 관리자 feature read에 더하지 않는다. 따라서 이전 기록의 “읽기 8개”와 이를 `/auth/me`로 맞춘 해석은 잘못된 분모였으며, 실제 admin read 분모는 7이다. 이 정정은 별도 shared auth endpoint를 없애거나 새 API를 만들지 않는다. 관리자 화면은 dashboard/users/videos/video-requests/comments **5개**(index redirect와 fallback은 별도 feature screen으로 세지 않음)다. 이 분모는 사용자 UI의 25개 unique route와 별도로 유지한다.

### 정책·상태 대조와 새 증거의 한계

핀 원본의 role/deactivate 동작과 기존 AIP 선언을 비교했다. SUPER_ADMIN은 활성 대상의 GENERAL↔ADMIN 지정을 할 수 있고, ADMIN은 GENERAL만 비활성화할 수 있으며 ADMIN/SUPER_ADMIN을 비활성화할 수 없다. 역할 변경은 SUPER_ADMIN 전용이다. 기존 정의가 이와 일치해 정책을 바꾸지 않았다. 추가된 fixture/HTTP/browser 코드는 SUPER_ADMIN의 GENERAL→ADMIN, ADMIN의 GENERAL 비활성화 및 ADMIN·SUPER_ADMIN 보호, ADMIN 직접 SDK 위조 대상/role 호출, 종료 요청 재거절 경계를 검증하도록 작성됐다. 그러나 이 Run의 ignored live test는 서버 시작 단계에서 generic `SERVER_CONFIG`로 중단됐고 policy probes와 DB assertions까지 도달하지 못했다. macOS Chromium preflight는 blocked였으며 local browser는 실행하지 않았다. 최종 CI도 실패했지만 로그와 artifact 본문이 접근되지 않아 이 추가 browser/DB 검증이 수행·통과됐는지 확인되지 않았다. 그러므로 이 slice의 허용·거부 assertion은 **구현된 검증 코드**이지, 통과한 실서비스/browser 증거가 아니다.

원본 경계 대조상 삭제된 영상을 다시 삭제하거나 PENDING이 아닌 종료 요청을 승인/거절할 때 원본은 변경을 거부한다. AIP 선언은 이미 종료된 요청에 대해 `from status = PENDING`을 요구한다. 새 코드에는 이미 REJECTED 요청 재거절이 `WIRE.EFFECT_TARGET_MISSING`이고 DB/화면 상태가 유지되는지 확인하는 probe가 있으나 live/browser 결과는 위와 같은 이유로 미확인이다. 원본 오류 envelope와 UI 문구의 완전한 동등성도 미검증이다.

### nullable host metrics와 부분 화면

`product/tests/fixtures/facereview-mysql/extensions/slice14h_dashboard.py`의 원본 경계에 따라 host metrics는 Linux 전용 provider가 값을 내지 못하면 `null`일 수 있다. 이는 정상 optional 입력이며 숫자 0의 의미가 아니다. 추가된 `dashboardStatus.ts`는 null metrics에서 임의 수치를 만들지 않고 “호스트 지표를 사용할 수 없습니다.” 부분 상태를 보여준다. 해당 helper 단위 시험은 통과했다. 기존 `e4f2efe`의 Linux browser는 metrics가 있는 경우의 대시보드 렌더만 확인했으므로 null branch의 mounted browser render는 미검증이다. 권한 slice를 막는 항목은 아니다. `/my/edit` 저장 뒤 빈 본문 screenshot은 원인과 API 계약이 별도 미확정이며 이번 admin 변경으로 해결됐다고 보지 않는다.

### 실행한 검사와 최종 상태

| 명령/검사 | 관측 결과 |
|---|---|
| `node --test porting/facereview/ui-overlay/tests/adminDashboardStatus.test.ts porting/facereview/ui-overlay/tests/adminAdapter.test.ts porting/facereview/ui-overlay/tests/adminReadAdapter.test.ts` | 24/24 통과 |
| `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` | 25 tests OK |
| `node --test spikes/spike-v6-transport/client/aggregate.test.ts spikes/spike-v6-transport/client/jobs.test.ts` | 21/21 통과(aggregate filter omission, UUID job scalar 포함) |
| `cargo fmt --all --check && git diff --check` | exit 0 |
| `cargo test -p aip-cli --features mysql --test mysql_facereview_admin_ui --no-run --locked` (합성 PostgreSQL build guard DSN, 별도 target) | compile 성공, test executable 생성 |
| ignored 실서비스 test `cargo test ... mysql_facereview_admin_ui -- --ignored --exact ...` | 실패: 서버 준비 단계에서 `SERVER_CONFIG`; policy probe 이전. 일반화 가능한 원인은 출력되지 않아 미확정 |
| macOS browser preflight | blocked (Chromium mach-register/mach-lookup); browser 미실행 |
| Verify `39939106930`, SHA `1e528aa…` | Failure, core exit 101; 원문 실패 step/assertion과 artifact 내부는 미확인 |

코드 commit/push는 `1e528aa4a2c47ca02a453c1c4f1f7c0075e5380f` 한 번이다. Verify 종료를 확인한 뒤에는 추가 코드 push를 하지 않았다. 이번에 바뀐 WBS source-of-truth와 coverage/다음 재개점은 이 문서, [상태 기록](../plan-docs/STATUS.md), [porting 정본](../plan-docs/porting/README.md), [overlay 안내](../porting/facereview/ui-overlay/README.md)에 이어 기록한다. PMS 7.2.9 `doing/90%`, 7.2.10 `doing/25%` 유지. 기간 7d 외, token reissue, 전체 사용자 25 route browser journey, profile after-save 결과 화면, model 권리 및 visitor policy는 미검증/보류로 남는다.

자원: 이 Run 소유 fixture container 세 개는 제거했고 label 조회 결과 잔존 없음. `aip-p730-*`와 다른 프로젝트 5173은 건드리지 않았다. 이 Run의 `/private/tmp/aip-adminui-20261023-{target,ui,sdk,npm-cache,definition.aip}` 및 `/private/tmp/aip-sdk-0.1.0.tgz`는 safety guard가 `rm -rf` 제거를 거부해 남아 있다. 다른 삭제 수단으로 재시도하지 않았으며 repo target은 없고 repo 크기는 약 1.6G다. 이전 Run의 `a14a5dd2` guard 거부 자료도 미확인 상태를 유지한다.

## 2026-10-23 관리자 에러 경계 live 검증 (코드 SHA `0acf32756a0e0cca38be9a83668022a686cadb52`)

**CI 실패 원문과 원인.** Verify `39939106930`(SHA `1e528aa`, attempt 1) `gh run view --log-failed` 원문을 보존했다. 실패 step은 13 `Verify real HTTP to Node and Python workers to MySQL`이고 `mysql_facereview_admin_ui.rs:488`이 `React admin browser journey failed (status Some(1))`로 panic했다. 브라우저 stdout은 `journey_stage=closed_request_refusal`, `RuntimeError: a non-PENDING request did not preserve the declared server transition boundary`다. 같은 run artifact의 `closed_request_refusal_code`는 `WIRE.INVALID_STATE`였고 서버 로그도 `/apply`에 `WIRE.INVALID_STATE`를 남겼다. 브라우저 시험이 `WIRE.EFFECT_TARGET_MISSING`을 기대한 것이 원인이다(시험 기대값 오류, 서버 정책 변경 아님). 그 run의 `mysql_job_claim_failed`(`WIRE.DB_UNAVAILABLE`) 로그 줄은 통과한 시험 구간에도 찍혀 이 실패의 원인이 아니다. 권한 assertion, flaky, `SERVER_CONFIG`가 아니었다.

**수정(직전 Run들의 작업 트리 변경을 보존·검증해 한 커밋으로).** 기대값을 `WIRE.INVALID_STATE`로 정렬, 이미 종료된 요청의 승인 job은 `WIRE.EXTENSION_ERROR`, 삭제 영상 재삭제는 `WIRE.MISSING_TARGET`로 단정, 대상 ADMIN 행 불변을 DB에서 확인(`mysql_facereview_admin_ui.rs`), null host metrics 타입 유지, 세션 단위 SDK client provider(`sessionClient.ts`), job 종결 시 읽기 캐시 무효화(`transport.ts`), 실패 시 screenshot과 `failure_line` 기록.

**로컬 실행(Claude 엔진 Mac 호스트, run-owned MySQL 8.4·Mongo 8.0·Redis 7.4 컨테이너 `aip-wbs729c-*`).** `sandbox-exec` probe는 exit 0이었고 이전 `SERVER_CONFIG`(Codex nested sandbox의 worker probe 거부, 위 213/861 기록)는 재현되지 않았다. 브라우저 없이 `cargo test -p aip-cli --features mysql --test mysql_facereview_admin_ui --locked -- --ignored --exact facereview_admin_console_runs_through_the_generated_sdk_and_the_servers_own_policy` → `1 passed ... finished in 5.34s`. 같은 시험을 고정 admin UI(ab0386c clone에 overlay, `tsc -b` exit 0, vite build, Playwright 1.64.0 Chromium 1248, UI port 5199)로 → `1 passed ... finished in 19.83s`, 브라우저 결과: 페이지 오류 0, `dashboard=rendered_partial_host_metrics_unavailable`(null 분기의 mounted render 확인), 위 에러 코드 3종, ADMIN의 FORBIDDEN 4건. 단위: `node --test` sessionClient/adminReadAdapter/adminAdapter/job-cache/jobs/aggregate 52/52, `python3 -m unittest product.tests.test_facereview_ui_browser_evidence` 25 OK, `cargo fmt --all --check`와 `git diff --check` exit 0. 원문은 이 Run outbox의 `local-admin-live-*.log`, `local-browser/`.

**CI (같은 SHA, 로컬과 구분).** Verify `39947955420` attempt 1 `completed success`(같은 SHA의 Linux worker jobs, Linux cgroup v2, External brokers, Object storage, Database contracts, App packages도 success). artifact `admin-browser-result.json`: `journey_stage=admin_permission_denials`, 페이지 오류 0, API 요청 148, `ok:false` 6건은 모두 의도한 거절(`WIRE.INVALID_STATE` 1, `WIRE.MISSING_TARGET` 1, `WIRE.FORBIDDEN` 4), `dashboard=rendered`(Linux 전체 지표). 이 run 종료 전 추가 push는 없었다.

**정책 확인 범위.** 고정 원본 `ab0386c` 대조: SUPER_ADMIN의 GENERAL→ADMIN 지정(브라우저 확인), ADMIN은 GENERAL만 비활성화(확인)·ADMIN/SUPER_ADMIN 대상과 role 위조 4건 거부(확인), 종료 요청 재결정과 삭제 영상 재삭제 거부(확인). 거부 코드는 AIP 서버 선언의 코드이며 원본 REST 오류와 같다고 주장하지 않는다.

**미검증·보류(`0acf327` 시점; ADMIN→GENERAL, 7d 외 기간, 토큰 reissue는 아래 후속 절에서 닫음).** SUPER_ADMIN의 ADMIN→GENERAL 방향, 기간 7d 외, 토큰 reissue, 원본 오류 문구의 동등성, 사용자 25 route 전체 journey, profile after-save 빈 화면(별도 미해결), model.h5 권리와 visitor 정책(보류). PMS 7.2.9 `doing/90%`, 7.2.10 `doing/25%`는 올리지 않았다.

**자원.** 이 Run이 만든 `aip-wbs729c-{mysql,mongo,redis}` 컨테이너, `/private/tmp/aip-wbs729c-*`(clone, SDK, venv, npm cache, 스크립트)와 재생성한 `/private/tmp/aip-sdk-0.1.0.tgz`를 삭제했다. `aip-wbs729b-*`(이전 Run 라벨), `aip-p730-*`, 이전 Run의 `/private/tmp/aip-adminui-20261023-*`, 다른 프로젝트 5173은 건드리지 않았다.

### 후속: ADMIN→GENERAL, 7d 외 기간, 토큰 reissue (코드 SHA `b5cd719d76e6af9d94f889c4899ab3ddea260e16`)

**원본 대조(`ab0386c`, 읽기만).** `UsersPage.tsx`는 SUPER_ADMIN에게만 `ADMIN 지정`/`ADMIN 해제` 버튼을 보이고 해제는 `changeUserRole(id, 'GENERAL')`이다. `DashboardPage.tsx`의 `PERIOD_OPTIONS`는 7d, 30d, 3m, 1y, 3y 다섯 개이고 `getSignupTrend(period)`가 이를 보낸다. `api/index.ts` 인터셉터는 응답이 HTTP 401이면 `POST /v2/auth/reissue`(HttpOnly cookie)를 한 번 부르고 재시도한다. 현재 SDK 경로는 `adminReadAdapter.ts`의 `PERIODS`(7d 7일, 30d 30일, 3m 90일 주 단위, 1y 365일 월 단위, 3y 1095일 월 단위)와 `authAdapter.ts`다.

**결함 발견(실제 브라우저 증거).** 서비스를 access TTL 20초로 띄우고 SUPER_ADMIN으로 로그인한 뒤 만료를 기다려 `회원 관리` 링크를 눌렀다. 서버는 만료된 token을 HTTP 401이 아니라 **HTTP 200과 `{ok:false, code:"WIRE.TOKEN_EXPIRED"}`**로 답했고(`wire_codes.rs`가 이 code를 reauthenticate로 분류), `authAdapter.ts`는 `status === 401`만 보고 reissue했으므로 reissue가 한 번도 일어나지 않았다. 수정 전 어댑터로 같은 여정을 실행한 결과(outbox `live-before-fix.log`, `browser-before-fix/`): `TOKEN_EXPIRED` 응답 2건, `/auth/reissue` 0건, 화면은 `불러오는 중...`에 머물렀다. 이 transport는 member overlay와 공유하지만 member 화면의 만료 동작은 따로 재현하지 않았다(미검증).

**수정(시험 선행).** `authAdapter.test.ts`에 5개를 먼저 추가해 5 fail을 확인했다(`ℹ pass 5 / fail 5`). 그 뒤 `accessTokenRefused()`가 HTTP 401 또는 8 KiB 이하 JSON 응답의 `ok:false`와 `WIRE.TOKEN_EXPIRED`/`WIRE.UNAUTHENTICATED`를 reissue 신호로 보도록 고쳤고, 동시 만료는 하나의 reissue를 공유하며, 거절된 reissue의 wire code를 `CLIENT.PROTOCOL_ERROR` 대신 그대로 올린다. 결과 `node --test .../authAdapter.test.ts` `pass 10 / fail 0`, overlay 시험 전체와 transport 시험 87/87. 다른 wire 거절(`WIRE.FORBIDDEN`, `WIRE.INVALID_STATE`), JSON이 아닌 본문, 큰 성공 본문은 reissue 없이 그대로 돌려주는 것도 시험한다. API mock·단정 완화·원본 정책 변경은 없다.

**검증 설계.** (1) 시드 계정의 `created_at`을 오늘/3일/15일/60일/200일/800일 전으로 고정해 기간별 버킷 합이 7d 2, 30d 3, 3m 4, 1y 5, 3y 6이 되게 했고, 각 버튼 클릭 뒤 차트의 점 개수가 서버에 보낸 버킷 요청 수와 같고 응답 합이 기대값인지 단정한다. (2) Alice를 DB에서 ADMIN으로 시작시키고(`ADMIN|0` 확인) SUPER_ADMIN이 원본의 `ADMIN 해제`를 눌러 GENERAL이 되는지, 종료 후 DB가 `GENERAL|0`인지 확인한다. 일반 ADMIN 세션에는 역할 버튼이 없다. (3) 새 `facereview-ui-admin-reissue-browser.py`가 TTL 20초 서비스에서 만료를 두 번 기다리며 링크만 눌러 각 회차에 거절 요청 1, `/auth/reissue` 200 정확히 1, 이후 요청 ok, 새 token 저장(SHA-256 비교, token 원문은 기록하지 않음)을 단정한다. SDK는 단일 in-flight 요청을 공유하므로 대시보드 첫 로드의 7d는 요청 수가 흔들려(관찰: 13 또는 14 버킷) 합 단정에 쓰지 않고, 가장 마지막에 조용한 페이지에서 7d 버튼을 다시 눌러 단정한다.

**로컬 실행(Claude 엔진 Mac 호스트, run-owned MySQL 8.4·Mongo 8.0·Redis 7.4, Playwright 1.64.0 Chromium, UI 5199).** `cargo test -p aip-cli --features mysql --test mysql_facereview_admin_ui --locked -- --ignored --exact facereview_admin_console_runs_through_the_generated_sdk_and_the_servers_own_policy --nocapture` → `1 passed ... finished in ~80s`를 커밋된 최종 스크립트로 연속 3회 통과(outbox `live-final-b1..b3.log`; 그 이전 스크립트 변형의 실패·통과 로그는 `live-run*.log`, `live-flaky*.log`, `live-final*.log`). 앞선 실패는 모두 원인을 확인해 고쳤다: 차트가 배치로 불러와 networkidle이 먼저 끝남, sync API가 네트워크 이벤트를 지연 전달함, 7d 대시보드 첫 로드가 in-flight 공유로 13/14 버킷. 로컬 `cargo fmt --all --check`, `git diff --check` exit 0, `python3 -m unittest product.tests.test_facereview_ui_browser_evidence` OK(26).

**CI(같은 SHA, 로컬과 구분).** Verify `39957123455` attempt 1 `completed success`, 같은 SHA의 나머지 6개 workflow도 success. artifact `admin-browser-result.json`: 페이지 오류 0, 요청 248, `dashboard=rendered`(Linux), `signup_trend_period_sums` 30일 3 / 3달 4 / 1년 5 / 3년 6 / 7일 2, `super_admin_demoted_admin_to_general`, `admin_sees_no_role_buttons`, ADMIN의 보호 대상 FORBIDDEN 4건. `admin-reissue-result.json`: TTL 20, 2회차 모두 `refused_requests 1`, `reissue_requests 1`, `requests_after_reissue_ok 3`, `new_access_token_stored true`, 페이지 오류 0, 요청 50, `tokens_recorded false`. 같은 run의 member add/remove/profile 여정은 `completed`/`completed`/`profile_fields_confirmed`로 공유 transport 수정의 회귀가 없었다(profile after-save 빈 화면은 별도 미해결로 그대로). 추가 push는 CI 종료 전에 하지 않았다.

**아직 미검증(이번 slice 밖 또는 근거 부족).** 거절된 reissue(refresh token 만료·폐기) 뒤 로그인 화면으로의 이동은 단위 시험(`unauthorized` dispatch)까지만 확인했고 브라우저 여정은 만들지 않았다(원본 UI 조작으로 refresh token을 만료시킬 수 없음). 원본 오류 문구와의 동등성, 사용자 25 route 전체 journey, profile after-save 빈 화면(별도), model.h5 권리와 visitor 정책(보류), member 화면의 만료 reissue 재현. PMS 7.2.9 `doing/90%`, 7.2.10 `doing/25%`는 올리지 않았다. 분모는 관리자 feature API 13개(읽기 7/쓰기 6), shared auth 4개, 화면 5개, 사용자 고유 경로 25개로 유지한다.

**자원.** 이 Run의 `aip-d19e-{mysql,mongo,redis}` 컨테이너와 `/private/tmp/aip-d19e-*`(clone, SDK, pack, npm cache, venv, 스크립트)를 삭제했고 라벨 조회 0건, 5199 listener 없음. `aip-wbs729b-*`, `aip-p730-*`, 이전 Run의 `/private/tmp/aip-adminui-20261023-*`, 다른 프로젝트 5173은 건드리지 않았다.

### 2026-10-23 member profile 렌더 및 token reissue 브라우저 진단 (코드 SHA `066187ad9d607b29e0770e1fadd07f12581c6644`)

**목표와 독립 결과.** 이 변경은 (1) `/my` profile 저장 뒤 빈 screenshot의 실제 렌더 원인과 (2) member UI의 access token 만료→shared `authAdapter` reissue→정상 화면 복귀를 같은 pinned 사용자 UI/fixture에서 각각 확인하도록 진단을 보강했다. 코드 SHA `066187ad9d607b29e0770e1fadd07f12581c6644`(Verify `39963850568`, attempt 1)는 `completed/failure`. step 10 `Verify compiler, runtime, product, worker, transport and SDK`가 실패했고 step 11~13은 skip이었다. 그러므로 이 run은 어느 목표의 live browser/DB 성공 증거도 제공하지 않는다.

**원인 증거의 경계.** 기존 증거는 profile 이름/장르 MySQL assertion 성공, 저장 후 screenshot의 `after_save_visible_text_length=133` 및 헤더만 보임이다. 이동 후 캡처 지연 가설은 앞선 반복 결과로 기각되었고 이 run은 이를 다시 원인으로 쓰지 않는다. 고정 원본(`facereview-front-joowon` SHA `2e7c021fa5abebd72278d841b8066a33d45cfbb3`)의 `mypage.scss` profile container과 내부 영역에 `fadeInUp` animation, `_motion.scss`에 opacity 0→1 keyframe이 있다는 것은 원본 코드에서 관측했다. animation/opacity가 screenshot 공백을 만든다는 건 검증 전 추론이며 원인 확정이 아니다. 새 계측은 computed style 및 조상 opacity, display/visibility/bounding box/transform, CSS animation·transition 상태, Web Animations API active animation, safe route, API body settled와 screenshot을 기록하고 실제 finite animation `finished`를 기다리도록 작성했다. 아직 브라우저에서 이 관측치가 수집되지 않았다.

**공유 auth/member 여정.** 같은 pinned member UI를 별도 20초 TTL 서비스에 연결하고, test-owned MySQL fixture에서 자연스러운 화면 버튼으로 만료 요청 후 reissue와 복구를 확인하도록 추가했다. 기록 대상은 `WIRE.TOKEN_EXPIRED`, `/auth/reissue` 요청 수, 후속 API 성공 및 새 token의 digest 확인뿐이며 token 원문은 남기지 않는다. 그러나 journey도 CI step 13 skip으로 미실행이다. 이전 `b5cd719`의 admin UI token reissue 성공은 별도 evidence이며 member UI 검증으로 간주하지 않는다. refresh token 거절 뒤 로그인 이동도 미검증 유지한다.

**명령과 출력.** `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` → `28 tests OK`; `node --test porting/facereview/ui-overlay/tests/authAdapter.test.ts` → `10/10 pass`; `python3 -c 'import ast; ast.parse(...)'` → profile browser script syntax OK; `cargo fmt --all --check && git diff --check` → exit 0; `env -u PGHOST -u PGPORT -u PGDATABASE -u PGUSER -u PGPASSWORD -u PGSERVICE -u PGSERVICEFILE AIP_TEST_DATABASE_URL='postgres://runner@127.0.0.1:5433/aip_test_ci' AIP_TEST_ADMIN_URL='postgres://runner@127.0.0.1:5433/postgres' cargo test -p aip-cli --features mysql --test mysql_facereview_auth --no-run --locked` → compile success, 실행은 안 함. configured `ttakkari_browser`에는 `about:blank` 탭뿐이라 페이지 계측은 수행하지 않았다.

**실패 조사 제한.** `gh run view 39963850568 --json status,conclusion,jobs`에서 Verify step 10 failure 및 후속 step skip을 관측했다. 실패 로그를 가져오려 한 `gh run view 39963850568 --log-failed`는 Codex nested command sandbox의 GitHub CLI cache 생성에서 `open /Users/winterholic/.cache/gh/run-log-39963850568-1792763124.zip: operation not permitted`로 실패했다. 해당 명령/환경의 캐시 쓰기 거부이며 CI 원인을 뜻하지 않는다. 같은 로그를 다른 경로·도구로 우회하거나 다시 시도하지 않았다. 따라서 step 10 원문 및 실패 원인 미확인, fixture DB assertion 미실행이다.

**다음 재개점.** 실패 원문을 승인된 경로로 확보해 step 10 원인을 좁힌 뒤 관련 회귀를 실행하고, 새 SHA에서는 같은 SHA CI 완료 후 browser/DB journey를 검토한다. 아직 제품 UI 최소 수정은 하지 않았다. 사용자 25 unique route coverage 미완료, 관리자 API 13(읽기 7/쓰기 6), shared auth 4, 화면 5의 분모는 유지한다. WBS 7.2.9 `doing/90%`, 7.2.10 `doing/25%`; model.h5 권리·visitor 정책은 보류다.

**후속 Verify attempt 2 (`066187a`, run `39963850568`).** 이 동일 SHA의 제한적 failed-job 재실행은 step10 성공(compiler/runtime/product/worker/transport/SDK, 19분), pinned member/admin 준비 step11–12 성공, step13 실패로 끝났다. artifact 보존 및 CI native fixture cleanup step14–16은 성공했다. 따라서 attempt1 step10의 원인은 확정할 수 없으며 attempt2 step10 통과만으로 flaky라고 부르지 않는다.

**member expiry 관측(artifact).** 안전 artifact `aip-run-39963850568-attempt-2-browser/profile/profile-browser-result.json`에서 visible login, `/session`, User와 UserFavoriteGenre reads, extension responses가 HTTP 200이다. TTL 만료 뒤 `/apply` 및 `/extension`이 HTTP 200 `WIRE.TOKEN_EXPIRED`를 반환했고 그 구간 `/auth/reissue` 요청은 없었다. script 결과는 `journey_stage=member_token_expired`, `failure_type=AssertionError`, `page_error_count=0`, `profile_ui={}`. 스크립트 순서에 따르면 expired refusal 검사 다음 “exactly one successful reissue” assertion에서 중단된 것으로 해석된다. artifact가 예외 문구와 response Content-Type을 저장하지 않아 adapter가 재인증으로 분류하지 않은 구체 이유는 확정되지 않았다. 화면에 reissue 후 복귀했거나 token이 교체됐다는 증거는 없다.

**profile 결과(별도 판정).** 동일 script가 TTL journey 성공 뒤 profile edit/save, after-save computed style/animation/route/visibility/API-settled 관측으로 이어지므로 앞 assertion에서 중단된 이 run에서는 profile journey가 실행되지 않았다. `profile_ui`가 비어 있고 `profile-before-save.png`, pre/post-animation screenshot은 artifact 목록에도 없다. DB fixture seed 및 profile API reads는 수행됐지만 profile save/DB post-save assertion은 실행되지 않았다. 그러므로 이전의 blank screenshot 원인은 미확정이며 `fadeInUp`은 여전히 코드에서만 관측된 미검증 가설이다.

**원문 및 범위 제한.** attempt1 `gh run view 39963850568 --log-failed`는 Codex nested command sandbox에서 GitHub CLI cache zip을 쓰다가 `operation not permitted`로 막혔다. 해당 cache/log path를 다른 경로·도구·엔진으로 우회하지 않았다. attempt2의 summary JSON과 안전 artifact는 읽었지만 raw workflow log는 읽지 않았다. attempt1 step10 원문·원인 미확인이다. 기존 same-SHA rerun 한 번으로 시도 제한을 지켰다.

**로컬 실행 근거.** browser evidence unittest 28개, `node --test porting/facereview/ui-overlay/tests/authAdapter.test.ts porting/facereview/ui-overlay/tests/mypageAdapter.test.ts` 12개, profile browser script AST parse, affected `cargo clippy -p aip-cli --features mysql --test mysql_facereview_auth --locked -- -D warnings`는 통과했다. isolated DB의 `absence_serializable`은 test 전 local port 5433 connection refused로 멈췄고, broad Node tests는 DB guard/PGHOST preflight, broad Python은 Codex nested sandbox uv cache permission으로 일부 시작 전에 실패했다. 이들은 CI step10의 실패 근거가 아니다. fresh checkout의 service/browser live journey 결과와 profile DB assertion은 없다. 원본 checkout을 수정하지 않았고 제품 UI도 수정하지 않았다.

**자원 및 진행.** CI step16 `Stop native driver fixtures` success; 이 Run 전용 isolated Cargo target 1.3 GiB 삭제. browser result와 contract evidence JSON은 이 Run outbox `aip-run-39963850568-attempt-2-browser/`에 있다. WBS 7.2.9 `doing/90%`, 7.2.10 `doing/25%`, 관리자 feature API 13(읽기 7/쓰기 6), shared auth 4, screen 5, member unique routes 25를 유지한다. 사용자 25 route 전수 미완료, model.h5 권리 및 visitor 정책 보류.

### 2026-10-23 attempt 2 failure branch 계측 및 독립 profile 여정

**출발한 실제 증거.** Verify `39963850568`, SHA `066187ad9d607b29e0770e1fadd07f12581c6644`, attempt 2는 `completed/failure`였다. `gh run download 39963850568 --name facereview-ui-browser-39963850568-2 --dir <Run outbox>/aip-run-39963850568-attempt-2-browser` 성공 후 받은 `profile/profile-browser-result.json`과 `contract-evidence.json`의 안전 필드에서 pinned UI SHA `2e7c021fa5abebd72278d841b8066a33d45cfbb3`, `journey_stage=member_token_expired`, `failure_type=AssertionError`, `page_error_count=0`, `profile_ui={}`, 만료 뒤 `/apply`·`/extension`의 HTTP 200 `WIRE.TOKEN_EXPIRED`, `/auth/reissue` 호출 0건을 확인했다. 해당 artifact에는 실패 assertion/line, response Content-Type, Authorization header 존재 여부, 실제 auth/SDK 호출 frame, body 종료 시각이 없다. 따라서 auth 분기 원인은 이 증거만으로 특정되지 않는다. attempt1의 step10 로그 cache 거부 경로는 다시 시도하지 않았다.

**원인 분리 계측.** `product/tests/facereview_ui_browser_evidence.py`는 제한된 trace만 남긴다: 요청 method/allowlist path/status, allowlist Content-Type, Authorization 존재 여부(boolean), 안전한 `ok`/wire code, envelope shape, 허용된 함수 frame, response-body 완료 및 `networkidle` 상대 순서. 알려지지 않은 path는 `other`로 축약한다. reissue branch candidate는 HTTP 200 JSON, `ok:false`, `WIRE.TOKEN_EXPIRED` 또는 `WIRE.UNAUTHENTICATED`, Authorization 있음, body 완료, `fetchWithRefresh` 경로가 모두 관측될 때만 인정한다. 코드 후보(`authAdapter.ts`가 401 또는 작은 JSON envelope를 분류하고, SDK custom fetch가 전달되는 구조)는 실제 trace 전까지 추론이며 결함 판정이 아니다. 계측 시험의 음성 대조는 Authorization 누락, `text/html`, fetchWithRefresh 경로 부재, body 미완료, body가 networkidle 뒤에 끝나는 각 경우를 branch candidate에서 제외한다. 원문 token, 쿠키, header 값 및 response body는 쓰지 않는다.

**profile과 reissue 분리.** profile은 일반 300초 TTL에서 로그인→profile 읽기/수정/저장→화면/렌더 계측을 독립 실행하며, member reissue는 별도 20초 TTL 서비스에서 자연 UI 만료 경로를 실행한다. Rust harness는 각 여정의 browser 결과와 MySQL persistence 결과를 서로 분리된 `profile-db-result.json` 및 `reissue-db-result.json`으로 남기고, 하나의 실패가 다른 여정을 건너뛰지 않도록 한다. profile 관측에는 computed style, 조상 opacity, animation/transition, route/visibility, API body completion 및 before/after screenshot이 포함된다. 기존 `fadeInUp` 관측은 원인 측정이 아니며, screenshot이 비어 있으면 성공으로 세지 않는다.

**로컬 검사와 한계.** `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` → `Ran 31 tests ... OK`; `node --test porting/facereview/ui-overlay/tests/authAdapter.test.ts porting/facereview/ui-overlay/tests/mypageAdapter.test.ts` → 12 tests, 12 pass, 0 fail; profile browser script AST parse → `profile browser AST OK`; `cargo fmt --all --check && git diff --check` → exit 0. Rust compile 첫 시도는 test-support build guard의 `AIP_TEST_DATABASE_URL is required`에서 끝났다. 명시적 local test DSN을 준 `env -u PGHOST -u PGPORT -u PGDATABASE -u PGUSER -u PGPASSWORD -u PGSERVICE -u PGSERVICEFILE AIP_TEST_DATABASE_URL='postgres://runner@127.0.0.1:5433/aip_test_ci' AIP_TEST_ADMIN_URL='postgres://runner@127.0.0.1:5433/postgres' cargo test -p aip-cli --features mysql --test mysql_facereview_auth --no-run --locked`는 compile success; live DB/browser는 실행하지 않았다. configured browser는 `about:blank`뿐이어서 로컬 UI 측정은 미실행이다. 새 CI run/attempt가 끝나기 전까지 profile 브라우저·DB, reissue 브라우저·DB와 원인은 모두 미검증이다.

**범위.** 위 코드는 계측과 journey 독립성만 변경하며 auth 정책이나 assertion을 완화하지 않는다. 실패를 잡는 음성 대조를 포함한다. 같은 SHA CI가 종료되기 전 새 코드를 추가 push하지 않는다. 사용자 고유 route 25개는 미완료, PMS 7.2.9 `doing/90%`, 7.2.10 `doing/25%`, model.h5 권리 및 visitor 정책 보류를 유지한다.

### 2026-10-24 실제 렌더 측정과 reissue 여정 선행조건 수정

**CI 범위.** SHA `91ca346314e1ac7bd42493acbb06f5fae10eda19`, Verify `39973060321` attempt 1은 `completed/failure`. step10 compiler/runtime/product/worker/transport/SDK success; step11 pinned member UI 준비와 step12 admin UI 준비 success; step13 real HTTP/worker/MySQL/browser failure; step14 artifact 보존과 step15–16 fixture logs/cleanup success. 나머지 6 workflows도 모두 success다. 안전 artifact는 outbox `aip-run-39973060321-attempt-1/facereview-ui-browser-39973060321-1/`에 있다. raw workflow log와 기존 거부 cache 경로는 다시 읽지 않았다.

**Profile: DB success와 screenshot failure를 분리.** `profile-db-result.json`: browser failure, database success; 저장 이름과 장르 2개 일치, 다른 회원의 `vlog` marker 유지. `profile-browser-result.json`: `journey_stage=profile_fields_confirmed`, `failure_type=AssertionError`, `failure_line=469`, 당시 assertion ID는 저장되지 않아 `unclassified`, page error 0. 실제 `profile-after-save-before-animation.png`는 화면이 어둡게 거의 보이지 않는다. 화면은 `/my`, API response bodies settled=true이고 profile target의 면적은 존재했지만 캡처 당시 4개 영역 opacity/effective opacity는 0–0.21, Web Animations API의 `fadeInUp`은 running이었다. `requestfinished` body 시간은 `networkidle` 완료 시각보다 약 76ms 먼저였다. 이후 `finish_profile_animations()` 측정에서 4개 필수 영역 opacity 1, visible, `visually_ready=true`가 됐다. 따라서 이전 blank screenshot은 profile CSS fade가 진행 중인 즉시 캡처 때문이라는 원인이 실제 computed style/timeline 및 screenshot으로 강하게 확인된다. 최종 after-animation screenshot은 line 469가 unrelated `other` 1400ms animations까지 실패로 판정해 생성되지 않아, 다음 run에서 최종 캡처를 확인한다. 새 gate는 관련 `fadeInUp`만 확인하고 screenshot은 assertion 전에 남기며, 4개 영역 readiness를 계속 요구한다.

**Member reissue: 아직 제품 분기 미측정.** `member-reissue-browser-result.json`: `journey_stage=member_token_expired`, `failure_type=AssertionError`, `failure_line=339`, assertion ID `unclassified`, `api_requests=[]`, trace requests 0. 원인은 기존 browser journey의 fixture coupling으로 설명된다: 바로 앞 profile journey가 같은 test-owned member의 이름을 `AIP 프로필 수정회원`으로 바꿨지만 reissue 여정은 원래 `AIP 프로필 표본회원` heading을 기대하고 그 assertion에서 API body/networkidle 수집 전에 종료했다. `reissue-db-result.json` browser/database 모두 failure, USED refresh row delta 0은 reissue 동작의 결과로 해석할 수 없다. 다음 변경에서는 저장된 이름을 기준으로 heading을 검증하고, 만료 요청의 body 수집/networkidle/auth trace 저장을 화면 heading assertion보다 앞에 둔다. 따라서 현재 artifact에는 Content-Type, Authorization 존재 여부, SDK/auth frames, body completion timeline이 없어 authAdapter/SDK 결함을 판정하지 않는다.

**로컬 증거와 다음 재개점.** 변경 작업 전 local `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` → 32 tests OK; authAdapter/mypageAdapter Node test 12/12 pass; profile script AST parse, `cargo fmt --all --check`, `git diff --check` exit 0. 직전 SHA Rust `--no-run --locked` build도 명시적 test DSN과 함께 성공했지만 live DB/browser 실행은 아니다. 새 코드 SHA CI가 종료되기 전 추가 push하지 않는다. 계측/report는 token, cookie, raw Authorization/header/body를 출력하지 않는다.

### 2026-10-24 CI step10 실패 위치 산출물 보강

SHA `6754227635153aaaf8b6eb58dc4fd6b661440785`, Verify `39977109763` attempt1은 step10 `Verify compiler, runtime, product, worker, transport and SDK`에서 failure, step11–13 skipped, step14 artifact action/step16 fixture cleanup success로 끝났다. `gh run download 39977109763 --dir <outbox>/aip-run-39977109763-attempt-1`는 `no valid artifacts found to download`를 반환했다. raw failure log의 기존 Codex cache-write 거부를 우회하거나 재시도하지 않았으므로 step10의 실제 원인은 미확인이다.

다음 검증 목적은 generic retry가 아니라 step10의 실패 command 특정이다. `tools/verify.sh`는 CI가 `AIP_VERIFY_FAILURE_FILE`을 설정했을 때만 `ERR` trap을 설치하고, 실패 시 exit status와 체크아웃 shell source의 `BASH_COMMAND` 한 줄(최대 500자)만 새 browser artifact directory 아래 기록한다. stdout/stderr, environment values, credentials, request/header/body를 쓰지 않는다. GitHub workflow는 step10 전에 artifact directory를 생성해 step14 upload가 이 locator를 보존하도록 변경했다. 로컬 `bash -n tools/verify.sh` 통과; 임시 shell failure control 출력 `exit_status=1`, `command=false`; `git diff --check` exit 0. CI가 locator를 만들기 전까지 원인은 미검증이며 이 실행은 다시 step10 통과를 가정하지 않는다.

### 2026-10-24 profile 원인 확인 및 member expiry 요청 확보

**CI 실행.** SHA `c64dd28988d516bfa5d4ea38e68a486b194b4754`, Verify `39979329689` attempt1은 step10 success, pinned UI 준비 step11–12 success, 실제 browser/worker/MySQL step13 failure, artifact 보존 및 native fixture 정리 step14–16 success다. failure locator는 step10 통과로 생성되지 않았다. browser/DB artifact는 outbox `aip-run-39979329689-attempt-1/facereview-ui-browser-39979329689-1/`.

**Profile 빈 screenshot root cause: measured.** profile browser와 DB가 모두 success. `profile-after-save-before-animation.png`은 `route=my`, API body settled=true인데 profile container opacity 0.0556, 나머지 3 영역 opacity 0, `fadeInUp` running. 완료 screenshot `profile-after-save.png`는 profile name, section controls/empty recent list, emotion chart skeleton 및 “회원정보가 수정되었어요”가 실제로 보인다. after-animation measurement에서 필수 영역 4개 모두 opacity/effective opacity 1과 `visually_ready=true`, 네 `fadeInUp` Web Animation은 finished다. response bodies는 networkidle 전에 끝났다. 따라서 blank after-save capture 원인은 `/my` API 지연이나 networkidle 경쟁이 아니라 screenshot 시점에 CSS fade가 완료되지 않았던 것이다. 수정 후 gate는 관련 profile fade 종료와 영역 readiness를 요구하고 최종 캡처도 저장한다. 코드/UI 정책 변경은 없다.

**Member reissue: 여전히 실제 auth 분기 미검증.** attempt1 reissue artifact의 `failure_assertion=member_navigation_exercises_expired_access_token`, `failure_line=352`, `journey_stage=member_token_expired`, page error 0. auth trace는 expired refusal 0, reissue 0, 그리고 만료 전에 끝난 login/session/read/extension 성공만 포함한다. MyPage route 이동은 됐지만 TTL 뒤 `/my`가 API를 새로 요청하지 않은 채 cached view를 보여줬다는 가설이 맞을 수 있으나, 이 artifact는 cache 원인을 직접 입증하지 않는다. DB `used_refresh_rows_added=0`는 token rotation 불발의 증거로 해석할 수 없다: TTL refusal 호출 자체가 관측되지 않았다. 그래서 authAdapter/SDK 분기 결함 결론은 내리지 않는다.

다음 reissue UI journey는 같은 member의 현재 saved name을 확인하고 edit 화면에서 값은 바꾸지 않은 채 실제 `수정` 저장(`/apply`) 버튼을 TTL 경과 후 누른다. 이렇게 하면 natural UI write로 만료 refusal과 reissue/retry가 발생하는지 측정할 수 있다. Playwright request events가 body 완료/networkidle까지 모인 뒤 assertion하며, `fetch` 및 XHR hook 설치 여부·호출 수·allowlist 함수 frames와 safe request path/content type/Auth-header-presence boolean을 같이 보존한다. 토큰/쿠키/header/body 값은 기록하지 않는다. profile screenshot 단계와 DB 결과는 현재처럼 별도 유지한다.

### 2026-10-24 member UI 만료 저장에서 auth reissue 실측

**CI 실행.** SHA `27db2af55eb65b71cdac65248d0dce04ee11ffa2`, Verify `39982476627` attempt1은 step10 compiler/runtime/product/worker/transport/SDK success, step11–12 pinned UI preparation success, step13 failure, step14 artifact/step16 fixture cleanup success다. 안전 artifact는 outbox `aip-run-39982476627-attempt-1/facereview-ui-browser-39982476627-1/`에 있다.

**Profile 결과.** 일반 TTL profile browser와 MySQL 둘 다 success. profile save 전 즉시 screenshot은 `/my`, API bodies settled, 4 content 영역 opacity 0–0.056·`fadeInUp` running으로 어둡다. animation 종료 후 screenshot은 profile heading, controls, empty video panel, emotion chart skeleton, success toast를 보여 준다. 종료 measurement의 4 영역은 opacity/effective opacity 1, `visually_ready=true`; API response bodies는 networkidle 전 끝났다. fade timing 원인을 실제 rendered screenshot/style로 확인했다.

**Member reissue 결과.** 20초 TTL 서비스의 UI 로그인 후 실제 edit page 저장 버튼으로 `/apply` 요청을 일으켰다. expiry trace는 `/apply`와 concurrent `/extension` refusal 2개, 모두 HTTP 200 `application/json`, Authorization header present=true, `ok=false`, `WIRE.TOKEN_EXPIRED`, body captured=true이며 body 완료 시각이 networkidle보다 앞선다. 바로 이어 `/auth/reissue`는 HTTP 200 `application/json`, Authorization header present=false, envelope ok=true 한 번이다. UI report: expired_refusals=2, reissue_requests=1, successful_retries=2, new_access_token_stored=true, `/my` route/필수 영역 visible, page errors 0. MySQL 별도 `reissue-db-result.json`: USED refresh row +1, rotation_observed=true. 따라서 이 자연 member write 경로에서는 만료 분기와 token rotation이 동작한다. 기존 failing Verify `39963850568` attempt2와 같은 failure의 차이는 아직 소급 증명되지 않았다: 과거 artifact에는 Content-Type/header presence/stack trace가 없었기 때문이다. 현재 관측은 이전 failure가 테스트된 브라우저 경로/요청 입력 차이였을 가능성을 지지하지만 이 attribution은 **추론**이다.

**미측정된 stack 및 suite의 별도 실패.** 이 run의 init-script trace 결과 `fetch_hook_installed=false`, fetch/XHR observation 0, `fetch_frames=[]`라서 실제 JS 함수 경로는 아직 측정되지 않았다. 원인은 Playwright `add_init_script`에 `() => {...}`를 넘겨 함수만 정의하고 invoke하지 않았던 것이다. 다음 변경은 IIFE로 감싸 실행하고, unit test가 init script IIFE 형태를 검증한다. 이 init bug 때문에 trace의 branch candidate 수는 0으로 계산되지만 request/response와 DB rotation은 별도로 측정됐다. 전체 step13 failure는 target reissue browser+DB success 후 실행된 기존 add-bookmark subjourney가 `journey_stage=bookmark_settle`에서 실패한 데서 왔다. 화면 screenshot에는 저장된 bookmark가 보이나 lifecycle capture array가 비어있어, 이 별도 failure의 원인도 미확정으로 둔다. 이 task는 add-bookmark behavior를 확대 수정하지 않는다.

### 2026-10-24 IIFE 계측 SHA의 Verify step10 실패 및 focused UI 실행 경로

**실행 결과.** SHA `b6719ad1520dfb4119566fc01d0fc6c96718ad9a`, Verify `39985750203` attempt1은 `completed/failure`. step10 `Verify compiler, runtime, product, worker, transport and SDK` failed; pinned UI preparation steps11–12와 real HTTP/worker/MySQL step13 were skipped. Artifact preservation step14, native fixture logs step15, native fixture cleanup step16, checkout cleanup step31 succeeded. `gh run list --commit b6719ad...`의 다른 workflow 다섯 개(Linux cgroup v2, External brokers, App packages, Object storage, Linux worker jobs)는 모두 success.

**실패 위치의 증거 범위.** `gh run download 39985750203 --dir <outbox>/aip-run-39985750203-attempt-1`로 받은 safe artifact의 `core-verify-failure.txt`는 `exit_status=101` 및 `cargo test --manifest-path "spikes/$engine/Cargo.toml" --locked --target-dir target`만 기록한다. 명령이 for-loop 안에서 실행되므로 checked-in source와 locator만으로는 어떤 `$engine` 반복에서 실패했는지 알 수 없다. stderr/stdout와 실제 failing test도 artifact에 없고 읽지 않았다. 이것은 step10의 Rust test command 경계에 대한 사실이지, 어떤 test crate의 원인도 아니다. 이전 GH CLI run-log cache write 거부 경로는 다시 시도하지 않았다. 같은 SHA CI를 무목적 재실행하지 않는다.

**목적이 제한된 다음 실행 경로.** 같은 SHA에서 UI stack을 수집할 필요가 남아 있어 `.github/workflows/verify.yml`에 수동 `workflow_dispatch` input `member_ui_diagnostic`을 추가했다. true일 때만 worker isolation 및 broad `tools/verify.sh` gate를 건너뛰고 `cargo build -p aip-cli --locked` 후 기존 pinned React/admin UI 준비와 `scripts/verify-mysql-worker-e2e.sh run-ci`를 실행한다. run은 기존 safe browser artifact upload와 run-owned fixture cleanup을 사용한다. 기본값 false이며 push/PR에서는 기존 broad gate가 그대로 실행된다. 이는 제품 auth/UI 동작이나 assertion/policy를 바꾸지 않는 계측 전용 실행 모드다.

**검증과 상태.** 다음 push 전에 workflow YAML 구조·expression 및 diff 정적 검사를 수행하고, 새 SHA의 자동 Verify가 종료된 뒤 목적이 분명한 dispatch를 한 번 실행한다. 그 artifact에서 `fetch_hook_installed`, safe allowlisted JS frames, Content-Type/envelope, Authorization 존재 boolean, response body와 networkidle timeline, profile/reissue browser와 DB 상태를 각각 확인한다. 해당 focused run이 없으므로 현재 branch stack은 미확정이며, 399824의 실제 success와 399638 attempt2의 과거 failure를 구분해 기록한다. 이 Run의 outbox locator는 `aip-run-39985750203-attempt-1/facereview-ui-browser-39985750203-1/core-verify-failure.txt`다.

### 2026-10-24 member reissue 종료 경계와 focused contract 정정

**두 기존 실행의 확인 범위.** Verify `39988060660`, SHA `17d00728820c5852b566c4cf8991779b28e7cf36`, attempt1은 step10 broad gate success, step14 HTTP/worker/MySQL/browser failure, artifact preservation success다. Safe artifact `artifact 12519325002`를 현재 Run outbox의 `aip-run-39988060660/`에 다운로드했다. Member browser JSON은 completed, refusal 2/reissue 1/retry 2, page errors 0이다. 별도 reissue DB JSON은 `database_status=success`, USED refresh row +1, rotation=true지만 `browser_status=failure`다. 같은 run의 profile browser/DB는 각각 성공했다. add bookmark는 별도 `bookmark_settle` AssertionError, lifecycle rows 0이다. 이 실패들을 auth policy 원인으로 합산하지 않는다.

Focused Verify `39994997512`, 동일 SHA attempt1은 API job steps 11–13(build/UI 준비) success, step14 failure, artifact total_count 0, upload `No files were found`다. 따라서 해당 run에서 profile/reissue browser 여정이 실행됐는지는 미확인이다. API job 결과는 새 Run에서 `gh run view 39994997512 --json ...`로 재확인했다. raw GH run-log 및 거부된 cache 경로는 열지 않았다.

**원인 후보와 최소 수정.** `product/tests/facereview-ui-profile-browser.py` reissue branch는 completed JSON을 쓴 뒤 `return 0`으로 Playwright scope를 빠져나오고 공통 `finally`가 살아 있는 browser를 다시 닫았다. profile branch는 context/browser 종료와 참조 해제를 scope 안에서 먼저 한다. 이 lifecycle 비대칭은 report success와 프로세스 failure를 함께 설명할 수 있는 코드 기반 가설이다. 성공 report 이후 driver-scope 종료가 예외를 내는 browser-free negative control과 source ordering regression test로 검증했지만, 399880에서 실제 종료 예외가 났다는 CI 관측은 없다. 최소 수정은 reissue도 report 뒤 scope 안에서 context/browser를 닫고 browser 참조를 비우게 한다. close 예외는 성공으로 삼키지 않는다. authAdapter/SDK 정책은 변경하지 않았다.

**Focused workflow 계약.** 기존 `member_ui_diagnostic`은 Mongo/Redis native fixtures와 broad gate를 건너뛰면서 전체 `run-ci` suite를 호출했다. 그 suite는 member MySQL/browser 뒤 recommendation, watch, external effects, jobs, admin을 더 실행하고 Mongo/Redis endpoints를 필요로 한다. 설정된 URL과 fixture startup 순서에서 이 조합은 불일치한다. 이는 source에서 확인한 구조이고, artifact 없는 `39994997512`의 첫 failing command가 Mongo/Redis였다는 주장은 추론으로도 확정하지 않는다. 새 `run-ci-member`은 기존 run-ci의 worker-write와 member-auth/UI 검사까지만 실행한 뒤 성공 종료한다. 기본 `run-ci`는 그대로 full suite이며 fixture gates도 유지한다.

**초기부터 남는 실패 locator.** workflow 시작 시 browser artifact directory와 `verify-failure.txt`를 먼저 만든다. workflow/script의 주요 stage에서 stage label을 갱신하고, 실패 시 checkout shell의 literal `BASH_COMMAND` 및 exit status를 최대 500자로 기록한다. 명령 출력·환경값·token·cookie·DB URL은 저장하지 않는다. 이후 artifact upload는 이 locator와 browser outputs를 함께 보존한다. 이는 테스트 전에 실패한 setup도 stage로 구분하고, 무파일 업로드를 피한다.

**검증.** TDD RED에서 lifecycle scope test와 locator negative test 각각 실패 확인 후 수정했다. 최종 `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence`는 38 tests OK; browser script AST parse, Ruby YAML syntax parse, `bash -n scripts/verify-safe-locator.sh scripts/verify-mysql-worker-e2e.sh tools/verify.sh`, `git diff --check`도 통과했다. CI/browser/MySQL은 이번 Run에서 실행하지 않았다. 새 코드 push 및 자동 Verify 결과는 follow-up으로 추가한다.


### 2026-10-24 Verify 40003138384 회원 증거 확인과 일반 E2E 분기 복구

`gh run view 40003138384 --json headSha,status,conclusion,attempt,jobs`를 직접 조회했다: 코드 `d29c014146fa557acf6983208e0aa6f988af4e98`, attempt 1, completed/success. Artifact API `12524337062` (`facereview-ui-browser-40003138384-1`)를 현재 기획 Run outbox에 다운로드해 JSON을 검사했다. Profile browser/DB status 모두 success, 저장 이름·장르 2개·다른 회원 marker 보존 true, after-animation 필수 영역 4개 visually_ready=true다. Member reissue browser/DB status 모두 success, USED refresh row +1, expired refusals 2, reissue 1, fetch hook 설치 true/관측 17개, allowlisted frames `send`/`fetchWithRefresh`/`postWithToken`를 확인했다. 이전 browser JSON completed 대 프로세스 failure 불일치는 이번 실행에서 재현되지 않았다. 이 사실은 과거 CI에서 종료 예외가 발생했다는 소급 증명이 아니다.

북마크 add/remove는 모두 completed, page error 0, failure_type=null이다. Add lifecycle은 요청/응답/finished 23개, 실패 0, unfinished 0이다. Remove는 요청 22개/finished 20개이며 종료 시 `/extension` 2건 unfinished가 있어 모든 요청 종료를 주장하지 않는다. Generic `verify-failure.txt`의 command/exit_status=pending은 실패 관측값이 아니며 실제 GitHub job 결과와 각 browser/DB JSON을 기준으로 판정한다.

**일반 실행 범위 회귀를 발견했다.** Workflow의 실제 run은 dispatch input과 무관하게 `run-ci-member`였다. 따라서 이 자동 Verify의 broad gate 및 member UI/DB 성공은 실제이지만 recommendation/watch/external effects/jobs/admin full MySQL E2E 성공으로 확대하지 않는다. 기존 shell full `run-ci`가 남아 있어도 workflow가 호출하지 않으면 일반 gate가 유지된 것이 아니다. 입력 없는 push/PR 및 false 수동 실행은 `run-ci`, true 진단만 `run-ci-member`를 선택하도록 최소 복구했다. 원인 대조 기준은 `.github/workflows/verify.yml`의 실행 경로다.

회귀 시험은 실제 workflow shell을 읽어 Bash로 빈 입력/false/true 세 경우의 선택 명령을 실행한다(실DB fixture 실행은 대체). 수정 전 빈 입력·false가 `run-ci-member`를 골라 FAIL 2건, 수정 후 evidence unittest 39 tests OK. Ruby YAML syntax parse 및 git diff check도 통과했다. Full suite 복구 SHA `50a012c`의 CI는 후속 Verify `40010266672`에서 success로 확인했으며(아래), 일반 CI 종료 전에 focused dispatch나 추가 push를 하지 않는다. UI 진단 전용 모드 자체의 새 실행은 이번 회원 성공 증거에 필요하지 않아 중복 dispatch하지 않았다.

WBS7.2.9 doing/90%,7.2.10 doing/25%,사용자25route 미완료,model.h5권리 및 visitor정책 보류는 유지한다. GH raw log cache 거부 경로는 접근하지 않았다.


### 2026-10-24 Verify 40010266672 전체 경로 확인과 회원 로그아웃 후속

**현재 실행 근거.** `gh run view 40010266672 --json headSha,status,conclusion,attempt,jobs`: SHA `50a012c9e844cfef3e5560838a61fc8bdd0add59`, attempt1, completed/success, core 19:41:55 UTC 종료. broad step11, real HTTP/Node/Python/MySQL step15, artifact16, fixture cleanup18 success. push 입력은 비어 있으므로 체크아웃 workflow의 실제 분기는 `run-ci`다. 직접 받은 safe artifact `12526429724` locator는 `stage=administrator console E2E test`, command/exit=pending이다. 이는 실패가 아니라 마지막 stage marker다. 마지막 관리자 stage까지 실행됐으며 step15 success이고, 고정 full script는 `set -euo pipefail` 아래 suite별 발견·정확한 test 이름·`1 passed; 0 failed` 및 admin-console 출력까지 필수 grep한다. 따라서 worker-write/member/recommend/watch/external effects/pool·trending·category jobs/admin lists/admin console의 일반 경로를 복구한 실행으로 판단한다. 개별 cargo stdout은 미열람으로 남기며 raw GH log cache 경로는 접근하지 않았다.

**직접 검사한 browser/DB JSON.** profile 두 status success, 이름·장르·다른 회원 marker true. reissue 두 status success, USED refresh +1, expired2/reissue1/retry2/new token stored true 및 SDK/auth frames. Admin checks는 5기간 합3/4/5/6/2, GENERAL↔ADMIN, 비활성화, approval job/거절, 영상·댓글 삭제, 권한거절4개를 기록한다. admin reissue 두 라운드는 각각 refusal1/reissue1/후속성공3/new token true다. bookmark add/remove는 completed/page error0이다. remove 종료 시 extension2 unfinished가 있어 모든 request 종료나 전수 UI 완료는 주장하지 않는다. profile PNG도 직접 열어 저장 이름·실제 영역 렌더를 확인했다. 원본 artifact는 이 Run outbox `verify-40010266672/`에 있다.

**다음 미검증 slice: 회원 로그아웃.** 기존 remove-bookmark 뒤 원본 헤더 `마이페이지로 이동`→원본 `로그아웃` 버튼을 누른다. `/auth/logout` 정확히1개, HTTP200+성공 body 수집, local auth의 signed-out/access-token-cleared/user-id-cleared boolean을 동시에 요구한다. 토큰·저장소 원문은 기록하지 않는다. MySQL의 해당 fixture 계정 revoked session row가1 증가하는지도 별도 `remove/logout-db-result.json`에 기록한다. 기존 bookmark assertions는 그 시점까지의 수집본에 적용해 logout 뒤 guest API와 섞지 않으며, 전체 lifecycle은 계속 보존한다. visitor landing의 허용 동작을 새로 정하지 않는다. 실제 browser/DB는 이 변경의 CI 전까지 미검증이다.

Evidence 판정기 negative control은 response 누락/중복·HTTP오류·wire오류·body미수집·인증 상태 잔류를 모두 거절한다. helper 부재로 실패를 확인한 뒤 추가했고 evidence unittest40 OK, member script AST OK, fmt/diff OK. Rust MySQL auth test는 PGHOST preflight 중단을 관측한 뒤 프로젝트 계약대로 PG*를 unset하고 explicit test DSN으로 `--no-run --locked --offline` 컴파일 exit0(6.11s)만 확인했다. local live/browser/DB는 미실행이다. WBS90/25와25route 분모,model.h5/visitor 보류 유지. 과거 실패 원인 소급 입증 없음.


### 2026-10-24 Verify 40017780097 로그아웃 성공과 좋아요 후속

`gh run view 40017780097 --json headSha,status,conclusion,attempt,jobs` 직접 조회: SHA `a87748d469e4f58fc07fd7396ed0821e812ba060`, attempt1 completed/success. Safe artifact `12530975676`을 현재 기획 outbox로 다운로드했다. `remove/remove-browser-result.json`: completed, page_error_count=0, member_logout.verified=true; logout_response_count=1, server_accepted/signed_out/access_token_cleared/user_id_cleared 모두 true. `remove/logout-db-result.json`: browser_status=success, database_status=success, revoked_session_rows_added=1. Profile 및 member reissue도 각각 browser/DB success. 마지막 locator는 administrator console E2E test이며, pending command/exit는 실패 결과가 아니다. Guest landing 정책이나 사용자 API25개 전수 완료를 뜻하지 않는다. 해당25경로 표의 logout 행만 새 증거로 갱신했다.

다음 작고 독립적인 미검증 동작은 `/v2/watch/like`의 원본 버튼이다. 원본 LikeButton은 비활성 `좋아요`, 활성 `좋아요 취소`와 aria-pressed를 제공하고 WatchPage는 낙관적으로 flip한다. 그래서 add/remove 각 bookmark phase 뒤 정확한 VideoLike create/remove `/apply` 성공 body가 끝난 것을 확인하고 reload한 뒤 반영된 버튼을 단정한다. Payload는 값을 저장하지 않고 정확한 SDK action만 allowlist로 분류한다. Rust caller는 두 phase 전후 MySQL video_like의 해당 계정·영상 행0→1→0, 다른 회원 like count 보존을 별도 `add/like-db-result.json`, `remove/like-db-result.json`으로 남기고 단정한다. 이전 logout 및 bookmark gate는 유지한다.

Helper 부재 RED 확인 후 allowlist와 evidence 검사를 구현했다. 정상 add/remove만 통과하고 미수집 body/실패HTTP/wire error/중복/다른resource/잘못된reload상태는 거절한다. Evidence unittest41 OK, browser AST parse, cargo fmt 및 git diff check 통과. 프로젝트 계약대로 PG*를 unset하고 명시적 test DSN을 둔 Rust MySQL auth test `--no-run --locked --offline` compile은 exit0(5.75s)이며 live DB가 아니다. 실제 like browser/DB 성공은 이번 변경의 CI가 종료된 뒤 판정한다. WBS90/25/사용자25경로 미완/model.h5권리·visitor 보류 유지, 기존 원본 checkout은 읽기만 했다. 과거399638/399880 실패 원인은 소급 판정하지 않는다.


### 2026-10-24 Verify 40025235103 좋아요 UI·DB 성공

`gh run view 40025235103 --json headSha,status,conclusion,attempt,jobs` 직접 조회: SHA `1e7fda7dca0c4971e11d8324d4aee4c0502500c3`, attempt1, completed/success. Broad step11/full HTTP-worker-MySQL step15/artifact16/fixture cleanup18 success, core 종료21:55:39 UTC. Safe artifact `12534242371`를 현재 outbox `verify-40025235103/`로 다운로드해 직접 검사했다. Locator는 마지막 administrator console E2E stage이며 pending command/exit는 실패가 아니다. 개별 cargo stdout과 raw GH log cache는 읽지 않았다.

Add/remove browser JSON은 각각 completed, page_error_count0, failure_type=null, member_like.verified=true, server_accepted=true, mutation_response_count1이다. Reload 후 active는 add=true/remove=false다. `add/like-db-result.json`, `remove/like-db-result.json` 모두 browser_status/database_status=success, member_like_rows는 각각1/0, other_members_preserved=true. 고정 Rust caller의 initial0 assertion 및 workflow step15 success를 함께 대조해0→1→0을 확인한다. PNG에서 하트 활성/비활성과 표시 합계2→1을 직접 확인했다. Player 재생/카메라 ML은 이 검증의 범위가 아니며 screenshot의 카메라 불가 상태는 그대로다.

이번 lifecycle은 add33/remove37 requests/responses/finished가 각각 같고 failed0,unfinished=[]다. 이전 run의 unfinished2를 이번 실행으로 소급 고치지 않는다. Logout verified=true, 별도 logout browser/DB success와 revoked session+1; profile/reissue 두 DB 결과도 browser/DB success, USED refresh+1을 확인했다. 해당 사용자25 unique API경로 표의 like 행만 새 증거로 갱신했고 전수완료로 세지 않는다.

다음 미검증은 원본 댓글 UI create/edit/delete, 이후 검색 및 계정 여정이다. 이번 실행에서는 새 코드 구현이나 이전41개 로컬 시험 재실행을 하지 않았다. 문서 diff check만 수행한다. 다음 단계는 새 컨텍스트에서 원본 UI·SDK payload·DB soft-delete 계약을 먼저 대조한다. WBS90/25/model.h5권리·visitor 보류 및 과거399638/399880 원인미확정 유지.


### 2026-10-24 원본 댓글 create/edit/delete UI·DB 여정 추가 (CI 대기)

원본 read-only checkout의 고정 SHA `2e7c021fa5abebd72278d841b8066a33d45cfbb3`, CommentSection/CommentItem/SomeIcon, writeAdapter의 bundle/create 및 Comment.edit/Comment.delete, Comment 정의의 소유자·활성상태·soft-delete 계약과 db-defaults를 대조했다. 앱 정책/원본UI/운영설정은 바꾸지 않았다. `facereview-ui-member-browser.py`에 comment-create/edit/delete 세 phase를 추가했다. 각 phase는 원본 로그인→fixture 카드→watch→안내 확인 경로를 거치고 원본 댓글 등록/더보기·수정/저장/삭제 확인 UI만 사용한다. create/edit의 내용·회원/영상 또는 edit/delete target ID를 boolean으로 대조하며 raw 댓글 내용·token은 계측 JSON에 기록하지 않는다. 정확한 단일 SDK mutation의 HTTP200/ok=true와 body 완료, reload 후 렌더·수정됨 표시 또는 비노출을 함께 요구한다. 기존 bookmark/like/logout 분기의 AST 26 statements는 동일하다.

Rust caller는 각 동작 직후 별도 `comment-*/comment-db-result.json`을 쓴다. Browser process status와 DB status는 분리한다. create에서 DB가 생성한 ID를 얻어 다음 두 phase의 예상 target으로 전달하고 같은 행의 영상·회원·내용·is_modified 0→1→1/is_deleted 0→0→1을 확인한다. delete 후에도 row가 남고 전체행은 initial+1이며 생성시각도 보존된다. 모든 무관한 댓글 행을 ID/영상/회원/내용/수정·삭제 플래그/생성·수정시각 전체 tuple로 비교한다(타회원 count만 비교하지 않음). 추가한 댓글이 보이는지뿐 아니라 기존 첫 댓글/두번째 댓글의 렌더도 요구한다.

로컬 실행: helper stubs에서 정상 판정 2개 FAIL→구현 후 evidence unittest43 OK. 다른 댓글 action이 같은 window에 섞이는 negative-control도 FAIL→거절 구현 후43 OK. `node --test porting/facereview/ui-overlay/tests/writeAdapter.test.ts` 6 pass; browser AST parse, cargo fmt --all --check, git diff --check 통과. Rust compile은 PG* unset, explicit loopback55432/aip_test_run DSN으로 `cargo test -p aip-cli --features mysql --test mysql_facereview_auth --no-run --locked --offline`, exit0/4.36s다. 처음 설정한 기본5432는 project guard에서 거절됐으며 기본 포트로 DB에 연결하지 않았다. 로컬 browser/live DB는 미실행; Linux 일반 Verify의 새 artifact 전까지 댓글UI 성공으로 세지 않는다. 최초실패 시 safe artifact/annotations만 조사하며 거부된 raw log cache 경로를 우회하지 않는다. WBS90/25,25경로 전수미완,model.h5/visitor 보류 유지. 성공 판정 후 search/계정 잔여를 이어간다.


### 2026-10-24 08:10 KST 댓글 Verify 성공 확인·검색 여정 추가 (검색 CI 전)

일반 Verify `40032647647` attempt1, 코드 `5741949eac1f13cd7e027a71b513b4a5b3922708`은 completed/success(2026-10-23 22:40:56 UTC 종료)다. `gh run view --json headSha,status,conclusion,attempt,jobs`로 broad step11·full HTTP/worker/MySQL step15·artifact16·cleanup18 success를 확인했다. Safe artifact `12537050658`을 현재 Run outbox에 다시 다운로드했고 comment-create/edit/delete JSON·reload PNG를 직접 검사했다. 세 단계 모두 completed/page_error_count0/member_comment.verified=true, 단일 mutation1/HTTP200/ok=true/body_captured=true, fixture target/content boolean과 contract 일치다. 각23 requests/responses/finished, failed0/unfinished0이다. PNG는 생성 댓글→수정 내용과 (수정됨)→삭제 후 기존 두 댓글만 표시한다.

각 `comment-db-result.json`의 browser/database success, same_comment_identity·target_content_and_flags_match·target_row_present·unrelated_rows_preserved=true, total_rows_added=1을 확인했다. 같은 ID의 is_modified0→1→1/is_deleted0→0→1이며 생성시각과 soft-delete 행보존·무관한 행 전체 snapshot 비교는 실행 SHA Rust caller와 대조했다. 25경로 표의 comments browser 행을 성공 근거로 갱신했다. 타회원 전체 행 보존은 댓글 범위이고 like는 기존 count 보존 범위다.

기존 회귀도 직접 검사했다: profile/reissue browser·DB success, USED refresh+1, add/remove like verified와 rows1/0·타회원 count 보존, bookmark completed/원본 reload PNG, logout verified·revoked+1. Add33/remove37 lifecycle failed0/unfinished0. Admin JSON은 dashboard/기간5/역할변경/승인·거절/삭제/권한거절4개를 기록하고 별도 reissue는 두 번 모두 reissue1·새 token 저장이다. Locator는 마지막 administrator console E2E stage이며 command/exit=pending은 실패가 아니다. 개별 cargo stdout은 미열람이다.

다음 검색 phase는 원본 Header 검색 버튼/입력/Enter→원본 SearchResultsSection을 사용한다. fixture 제목 검색1건·고유 absent query0건을 각각 제출/새로고침해 총4개 상태를 검사한다. 정확한 `Video.homeSearch` input(page1,size20,title,emotions empty)을 boolean으로 분류하고 단일 완료 body/contract/목록 길이·fixture 포함/렌더를 요구한다. raw 검색어를 네트워크 JSON에 저장하지 않는다. 검색에는 player 진입·업무 mutation이 없으며 종료되지 않은 API도 실패 처리한다. Rust는 로그인 세션 metadata와 별개로 11개 업무 테이블 모든 행·열의 검색 전후 snapshot 동일성을 검사한다. 원본 UI/policy/overlay 제품코드는 불변이다.

로컬: 새 helper stub에서 의도한 FAIL3→GREEN46, 기존 watch/comment/bookmark/like/logout branch AST 동일, readAdapter14 pass, browser AST·fmt·diff OK, PG* unset와 전용55432/aip_test_run DSN의 Rust auth `--no-run --locked --offline` compile6.27s exit0(실DB 아님). 실제 검색 browser/DB는 새 CI 전 미검증. 성공 후 계정잔여(check-email/signup/tutorial/email/password/confirmed withdrawal)의 원본 화면·기존 source overlay·메일 sink·DB/session 부수효과를 대조해 bounded 여정으로 이어간다. WBS90/25·25경로 전수미완·model.h5권리/visitor보류와 과거399638/399880 원인미확정 유지.


### 2026-10-24 09:10 KST 검색 Verify 성공·원본 비밀번호 계정 여정 추가 (password CI 전)

Verify `40038229142`, SHA `f436a06173d2207020f5e334d8caa5f3bc40e3f4`, attempt1 completed/success, core종료23:45:33UTC. Broad11/full15/artifact16/cleanup18 모두 success. Artifact `12539367749`를 이번Run outbox `verify-40038229142`에 다운로드해 `search/search-browser-result.json`, `search-db-result.json`, found/empty submit/reload PNG4개를 직접 검사했다. 4gate와 전체verified=true; 정확한 Video.homeSearch4개는 각각 HTTP200/ok/body_captured/search_input_matches/contract_matches_runtime=true. Found2개 length1/fixturetrue, empty2개 length0/fixturefalse다. PNG는 고양이 영상1카드와 검색결과없음 문구를 보여준다. Lifecycle22 request=response=finished/failed0/unfinished[], page_error0. DB는 browser/database success,11테이블 fullsnapshot 불변true이며 실행SHA caller와 대조했다. 25경로 search행을 성공 근거로 갱신했다.

회귀 JSON 직접검사: 댓글3phase verified, 각23finished/failed0/unfinished0, DB 동일ID·행+1/modified0→1→1/deleted0→0→1/무관한댓글fullsnapshot보존. Profile/reissue DBsuccess와 USEDrefresh+1 및 expiry2/reissue1/retry2·새token 저장·렌더준비true, like add/remove verified 및 DB1→0/타회원count보존, logout verified/revoked+1, bookmark add/remove completed. Admin dashboard/period5/역할·승인·거절·삭제·권한거절4개 및 별도 reissue2회도 기존 JSON 범위대로 확인했다. Locator는 마지막 administrator console E2E stage; command/exit=pending은 실패가 아니다. 개별 cargo stdout은 미열람이다.

원본 read-only UI에서 계정 호출처를 대조했다. MyPage 미인증 배너의 handleVerifyEmailClick/코드 modal의 handleVerifyCodeSubmit은 sendEmailVerification/verifyEmailCode를 호출한다. PasswordChangePage는 별도 이메일전송→verifyPasswordCode→reset_token→changePassword→원본 useLogout('/auth/1')다. AuthPage는 checkEmail/signup, TutorialPage의 완료·skip은 completeTutorial, EditPage 확인모달의 최종버튼은 withdraw로 연결된다. source 연결만으로 해당 화면을 검증됐다고 세지 않는다.

신규 `product/tests/facereview-ui-password-browser.py`는 전용 disposable 계정으로 원본 로그인·MyPage 버튼·PasswordChangePage code발송/입력·새password·자동logout·새password재login/reload를 실행한다. Code/reset_token은 메모리에서만 다루고 네트워크 계측은 path/method/status/성공body·payload-match·인증 boolean만 저장한다. Logout은 원본 authPost가 bearer 대신 refresh cookie를 사용하므로 cookie-presence만 요구한다. 실패 PNG 전에 모든 input을 지우고 raw exception/response를 출력하지 않으며 실패 stage/type/자체script line만 남긴다. Mailer sink는 일회성 run-owned loopback capability endpoint에서 password fixture 수신자의 code만 Python driver에 제공한다; 제품 API를 추가하지 않았다.

Rust caller는 기존 mail API시험이 끝난 뒤 fixture 전용계정과 다른기기 session을 준비한다. UI 성공과 DB 성공은 별도 password JSON으로 남긴다. Hash변경/scrypt형식·oldpassword거절/다른기기 bearer거절, password 변경·logout으로 revoked2/새login active1/challenge0, mail1회·해당수신자, 계정flags GENERAL:0:1:0 보존 및 target user만 제외한11개 업무테이블 fullsnapshot·다른계정session fullsnapshot 불변을 검사한다. Signup/tutorial/email verify/withdraw 화면 성공으로 확대하지 않는다.

Local evidence: helper stub FAIL1→전체unittest47 OK; 실제logout cookie 계약을 넣은 RED→GREEN도 확인. mypageAdapter node2pass, `mailbox_capability_reads_only_the_password_fixture_in_memory` 실loopback HTTP1pass/0.01s(다른recipient·잘못된capability는code비노출). Rust auth no-run offlinecompile2.50s exit0; 처음의 Vec 타입추론 E0282는 Vec<Value> 명시로 수정했다. PG* unset/전용55432/aip_test_run 계약 준수. 두 browser AST·기존member/search script byte동일·fmt/diff OK. 로컬 liveDB/browser는 미실행; 새 Linux 일반 Verify의 password artifact 전까지 계정UI는 미검증으로 둔다. 원본UI/overlay제품정책/원본5173·aip-p730·guard자료 불변. WBS90/25·사용자25경로전수미완·model.h5권리/visitor보류 유지.


### 2026-10-24 10:00 KST Verify40043525384 broad gate 실패와 clippy·nested locator 수정

일반 Verify `40043525384` attempt1, SHA `3678af624974325fb1ac082104426f869f06b20d`은 completed/failure(00:14:06UTC 종료)다. 최초 실패는 step11 broad compiler/runtime/product/worker/transport/SDK이고 React 준비13·admin준비14·실제HTTP/DB/browser15는 skipped다. Safe artifact `12539998150`에는 verify-failure.txt만 있고 stage=broad gate/command=bash tools/verify.sh/exit101이다. Annotations도 exit101 외 첫 Rust 오류를 특정하지 않는다. 비밀번호 browser/DB는 도달하지 않았으므로 관련25경로행은 pending을 유지한다. 거부된 raw GH log cache 경로를 우회하지 않았다.

로컬에서 변경된 MySQL auth test의 clippy를 좁혀 실행해 exclude_user.is_some() 뒤 unwrap()이 clippy::unnecessary_unwrap/-D warnings로 exit101 되는 결함을 직접 재현했다. match (table, exclude_user)로 바꿔 같은 SQL·parameters·snapshot 의미를 유지했다. 이후 CI와 같은 MySQL migrate/service/CLI all-target clippy는 explicit loopback55432/test DSN·PG*unset·--locked --offline에서 exit0/3.38s다. 원격 CI의 첫 Rust 오류 원문이 없으므로 이 결함을 과거 CI의 직접 원인으로 확정하지 않고 유력한 코드 기반 설명으로 둔다.

추가 계측 결함도 독립 재현했다. nested shell의 실제 inner failure locator를 부모 bash command/outer stage가 덮어쓰는 회귀 시험이 FAIL이었다. pending stage/start marker는 새 gate를 시작하게 두고, 이미 numeric failure가 기록된 경우 부모의 후속 non-pending write가 덮지 않게 했다. 자식 첫실패 보존과 다음stage reset을 검사했고 전체 evidence unittest49 OK다. tools/verify.sh의 DBguard/readiness/fmt/workspace/auth-service/PoC/MySQL clippy 앞에 단계 marker를 추가했다. 검사 명령·순서·범위·제품코드는 바꾸지 않았다. shell syntax/fmt/diff OK; 로컬 liveDB/browser는 미실행이다.

새 일반 CI로 비밀번호 원본 UI·DB 증거를 확보한다. 동일ref 진행중 push/dispatch/cancel·장시간대기·맹목retry 금지,45분timeout+3분 후속예약을 유지한다. 성공 뒤에만 해당25경로행 갱신 및 이메일검증/signup/tutorial/confirmedwithdraw 잔여로 진행한다. 컨텍스트54%의 연속성을 사용해 이번 조사·작은수정을 마친 뒤, 상태·변경·검증을 인계에 저장하고 새컨텍스트로 결과확인을 이어간다. WBS90/25·25경로전수미완·model.h5/visitor보류 유지.


### 2026-10-24 Verify40062036395 비밀번호 UI·DB 성공 및 MyPage 이메일 인증 증분

일반 Verify `40062036395` attempt1, SHA `08314567afc200377ae2d366209284f359827119`은 completed/success(core종료 2026-10-24 04:45:09UTC)다. Broad11·UI준비13/14·실HTTP/worker/MySQL15·artifact16·cleanup18 모두 success. Safe artifact `12548545988`를 현재 Run outbox `verify-40062036395`에 다운로드했고 password JSON2개·PNG4개를 직접 검사했다. `member_password` 전체6gate/verified 및 UI6boolean true, `/auth/password` POST1건·정확payload·완료된성공body, logout body/refreshcookie/clientclear, old/new login 성공과 새비밀번호 재login/reload를 확인했다. API27건 모두 finished/failed0/unfinished0·pageerror0. `/session`은 전체3회(최초login1/새login1/reload1)이며 재로그인 경합에서 보이던 중복2회가 없다. 첫 PNG는 fade 도중 어둡지만 코드 후 폼·로그아웃·최종profile PNG를 확인했고 비밀번호 입력·code·reset_token은 저장하지 않았다.

Password DB는 browser/database success 및 모든 boolean true다. Hash변경/scrypt, oldpassword거절, 타기기폐기, revoked2·active1/challenge0, mail1회·수신계정/template, flags GENERAL:0:1:0 보존 및 target user만 제외한11개업무테이블 fullsnapshot·다른계정session fullsnapshot 보존을 실행SHA caller와 대조했다. 사용자25경로 표는 비밀번호 여정의 send-code/code-exchange/password 세 행만 갱신했다. 이메일 verify/signup/tutorial/withdraw 화면 성공을 이 결과로 주장하지 않는다.

기존회귀 JSON도 직접검사했다. Profile/reissue browser·DB success/USEDrefresh+1 및 expired2→reissue1→retry2, like add/remove·DB1→0/타회원count보존·logout revoked+1, 댓글3phase·동일ID/softdelete/무관행보존, search4gate/11테이블보존이 유지됐다. Add33/remove37/comment각23/search22 lifecycle은 failed0/unfinished0이다. Admin 기존 dashboard/기간5·role/승인/거절/삭제/permission4 및 reissue2회를 JSON 범위대로 확인했다. Locator의 마지막 admin stage/command·exit pending은 실패가 아니고 개별cargo stdout은 미열람이다.

비밀번호 JSON의 `/apply` HTTP200/okfalse는3건이며 전부 `WIRE.NOT_FOUND`다. MyPage→getEmotionSummary→SDK writeExtension(User.myEmotionSummary)→/apply라는 source 경로와 일치하지만 현재 artifact에는 호출명이 없어 동일 호출이라고 확정하지 않는다. 다음 password/email JSON에 허용된 해당 호출명만 기록한다. 감정그래프 skeleton이나 이 오류를 성공으로 포함하지 않으며 별도 원인조사 잔여다.

다음 bounded 증분은 원본 MyPage 미인증banner→이메일인증모달→메일code입력→badge/banner제거→reload유지다. 방금 비밀번호를 바꾼 동일 disposable계정의 fresh browser를 쓰며 기존 run-owned in-memory메일sink를 재사용한다. DB는 인증flag0→1/GENERAL:0:1:1, target user의 인증flag·updated_at 외 모든열보존, challenge0/mail1회, active2/revoked2, 무관한11개업무테이블·타계정session fullsnapshot 불변을 별도 JSON으로 판정한다. 원본UI/제품정책 불변. Email UI·DB는 새 CI 전까지 미검증이며 signup/tutorial완료·skip/confirmedwithdraw는 다음 잔여다.

로컬: 새 email 판정기 stub의 AssertionError RED→GREEN, 전체evidence unittest50 OK/overlay node64 pass, AST/fmt/diff OK, MySQL migrate/service/CLI all-targetclippy6.50s exit0 및 Rust auth --no-run --locked --offline compile6.29s exit0. 로컬liveDB/browser는 미실행이다. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지. 같은ref Verify진행중 push/dispatch/cancel·장시간대기금지 및45분timeout+3분후속예약을 유지한다.


### 2026-10-24 Verify40065518651 이메일 인증 UI·DB 성공 및 가입/튜토리얼 증분

일반 Verify `40065518651`, SHA `22089a4e0def6cd9461420bf72c3a7990a8c46cf`, attempt1은 completed/success(core종료 2026-10-24 05:48:35UTC)다. Broad11·원본UI준비13/14·실HTTP/worker/MySQL15·safeartifact16·cleanup18이 success다. Artifact `12549937284`를 현재Run outbox `verify-40065518651`로 다운로드해 email JSON2개·PNG4개를 직접 검사했다. MyPage 미인증banner→코드modal→인증badge/banner제거→reload유지의 UI5boolean, member_email3gate·verified 모두 true다. 두 인증경로는 POST각1건/HTTP200/ok/body_captured/payload_matches/authorization_present true. API20건 모두finished/failed0/unfinished0/pageerror0. Code/password/token 원문은 산출물에 기록하지 않는다.

Email DB의 browser/database success 및 모든boolean true를 확인했다. Flags GENERAL:0:1:1, target인증flag·updated_at외모든열보존, challenge0/mail1회·수신계정/template, active2/revoked2, target user제외11업무테이블과타계정session fullsnapshot보존을 실행SHA caller와대조했다. 25경로표의 email verification/verify 두행을 성공근거로갱신했다. 가입·튜토리얼·탈퇴 성공으로 확대하지 않는다.

Password는 전gate·DBbooleantrue/28finished/failed0/unfinished0/pageerror0이다. 기존profile/reissue(DBsuccess,USEDrefresh+1,expired2→reissue1→retry2), like1→0/타회원보존/logoutrevoked+1, 댓글3phase동일ID·softdelete·무관행보존, search4gate·11테이블보존과 admin기존checks/기간5/permission4/reissue2회도 JSON범위대로재확인했다. Add33/remove37/댓글각23/search22 lifecycle도완료됐다. 마지막locator는admin stage/command·exit pending이며실패가아니다. 개별cargo stdout은미열람이다.

이번계측에서 password의4건/email의3건 `/apply` HTTP200·okfalse가 전부 write_extension=`User.myEmotionSummary`/error_code=`WIRE.NOT_FOUND`로 확인돼 호출대상은 확정됐다. 실패근본원인은 아직미검증이며 이메일인증과별도잔여로둔다. PNG의감정그래프 skeleton·일시적인0표시를 감정통계검증성공으로 세지않는다. 과거실행의호출대상을소급확정하지않는다.

다음 bounded증분은 원본AuthPage signup/checkEmail과 TutorialPage 완료·건너뛰기다. 서로다른2개disposable계정을 실제원본가입UI로만만들고(preseed없음), 각signup직후DB를검사한후fresh browser에서login→tutorial을실행한다. Check-email은 신규false→가입후existingtrue 분기/정확body를검사하고 signup은POST201각1건·정확name/genre/password boolean, 로그인폼복귀와genre선택PNG를남긴다. DB는server생성UUID·scrypt·GENERAL:0:0:0·장르animal/eating정확2행·active1/revoked0 및신규user/genre행외11테이블·타계정session보존을요구한다.

Tutorial은complete경로의step1/2/3와완료버튼 또는별도계정의skip버튼→단일User.completeTutorial/정확본인target·성공body·인증/contract→main/reload후실제profile재조회로user_tutorial1을요구한다. DB는GENERAL:0:1:0/target tutorialflag·updated_at외모든열·genre/타계정보존·active2/revoked0을별도JSON으로기록한다. 원본UI/정책불변. 새account UI·DB는CI전미검증이고 EditPage confirmedwithdraw는다음잔여다.

로컬실행: signup/tutorial판정기stub AssertionError RED2→GREEN, 전체evidence unittest52OK·adapter node64pass·AST/fmt/diffOK. Rustauth no-run --locked --offline compile3.81s exit0, MySQL migrate/service/CLI all-targetclippy -Dwarnings2.80s exit0. PG*unset/전용55432/testDSN 계약준수; 로컬liveDB/browser미실행. 기존password/email/member제품코드·판정범위는불변이고snapshot helper의새genre제외옵션은signup비교에만적용한다. WBS90/25·25경로전수미완·model.h5권리/visitor보류유지. 동일refVerify진행중push/dispatch/cancel·장시간대기금지,45분timeout+3분후속예약유지.

---

### 2026-10-24 Verify40068996762 가입 UI 성공·DB 판정기 조사

일반 Verify `40068996762`, SHA `68cdf255d738c7f7a977c9ae9bdbf8979cb4f8b9`, attempt1은 completed/failure(core종료 2026-10-24 06:27:30UTC)다. Broad11·UI준비13/14는 success, 최초 실패는 실HTTP/worker/MySQL step15다. Safe artifact `12551196219`를 현재Run outbox `verify-40068996762`에 다운로드했다. Locator는 stage=FaceReview member authentication E2E test, command=tee /tmp/aip-e2e-auth-run-"$tag".txt, exit101이다. raw GH log cache는 접근하지 않았다.

`account-complete/signup-browser-result.json`과 PNG2개를 직접 검사했다. 원본AuthPage 신규email→비밀번호확인→이름/먹방·동물선택→가입후빈로그인폼복귀→기존email분기는 completed/member_signup 전gate·verified=true다. Check-email은 POST200 신규false/기존true 각1건, signup은 POST201 1건이며 정확payload·완료성공body/3finished/failed0/unfinished0/pageerror0다. 그러나 signup DB는 failure: UUID/scrypt/name·초기flags/active1 revoked0/타계정session보존은 true지만 selected_genres_match와 unrelated_business_rows_preserved가 false다. Account-complete tutorial 파일과 account-skip 폴더는 없으며, 후단 admin도 실행되지 않았다. 성공 조건이 충족되지 않았으므로 checkEmail/signup/tutorial 25경로행은 갱신하지 않았다.

코드 기반 원인 두 가지를 좁혔다. (1) 장르열은 MySQL ENUM으로 eating이 animal보다 먼저 선언된다. ORDER BY genre는 선언순 정렬인데 판정기는 animal,eating을 요구했다. 기존 profile 검증과 같이 ORDER BY CAST(genre AS CHAR)로 바꿨다. (2) signup 전 snapshot은 query(text), 후에는 user/genre만 exec(binary)라 raw Value Debug 비교가 타입표현 차이를 잡는다. 로컬의 설치된 mysql_common0.37.3 실제 RowDeserializer를 오프라인 실행해 동일 SQL정수1이 text=[Bytes("1")], binary=[Int(1)]인 것을 재현했다. Helper의 무필터 조회도 exec로 통일했다. 모든행·모든열·순서 및 제외대상은 그대로며, 보존조건을 줄이지 않았다. 이 원인을 CI 실패의 최종 원인으로 확정하려면 새 CI DB결과가 필요하다. 당시 실제 DB가 보존됐다고 소급 주장하지 않는다.

앞단 password(28finished)/email(20finished)는 전gate·DBboolean true, profile/reissue/like/logout/comment3phase/search도 각 JSON 범위 성공을 재확인했다. Add33/remove37/comment각23/search22는 failed0/unfinished0/pageerror0이다. Password/email의 User.myEmotionSummary/WIRE.NOT_FOUND는 별도 잔여이며 graph skeleton을 성공으로 세지 않는다. 후단admin은 이번 artifact에 없으므로 미검증이다.

로컬검증: evidence unittest52 OK, cargo fmt/diff OK. PG*unset·explicitloopback55432/aip_test_run DSN에서 MySQL migrate/service/CLI all-target clippy -Dwarnings5.92s exit0, auth test no-run --locked --offline compile6.59s exit0. 컴파일은 실제 DB검증이 아니다. 기존 password/email/member/account browser scripts와 원본UI·overlay 제품코드·정책은 불변이다. Clone/DB/container/localbrowser 신규생성 없음. 새 일반 Verify 결과를 기다린다. 성공시 두 account phase의 JSON/PNG 전체와 기존회귀를 직접검사한뒤 관련25행만 갱신→EditPage confirmedwithdraw로 이어간다. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지.


### 2026-10-24 Verify40072496940 첫계정 가입 DB 성공·튜토리얼 reload 판정 수정

일반 Verify `40072496940`, SHA `ef828ac7a1dbb9b442fe4a97c1faef4e19f09031`, attempt1은 completed/failure(core종료 2026-10-24 07:22:58UTC)다. Broad11·UI준비13/14는 success, 최초 실패는 실HTTP/worker/MySQL step15다. Safe artifact `12552860245`를 현재Run outbox `/Users/winterholic/.ttakkari/data/outbox/a461ad09-aff0-4da4-b4f5-d65e56ee8ac4/verify-40072496940`에 다운로드했다. Locator는 stage=FaceReview member authentication E2E test, command=tee /tmp/aip-e2e-auth-run-"$tag".txt, exit101이다. Raw GH log/cache는 접근하지 않았다.

첫 account-complete의 signup JSON2개·PNG2개를 직접 검사했다. 신규false→기존true checkEmail 각1회·signup POST201 1회/정확payload·성공body/전gate·verified 및3finished/failed0/unfinished0/pageerror0다. DB도 browser/database success, 전boolean true: 서버UUID/scrypt/name·GENERAL:0:0:0/animal,eating 정확2장르/active1 revoked0/신규user·genre 외11업무테이블·타계정session 보존이다. 직전 false였던 selected_genres_match·unrelated_business_rows_preserved가 이번에는 true다. ENUM문자정렬·snapshot exec통일 이후 첫계정에서 재발하지 않았으나 과거 실행의 실제 행보존을 소급 보증하지 않는다.

Tutorial complete는 initial/second/final-step PNG3개와 account-failure PNG를 직접 검사했다. Step1/2/3·단일 User.completeTutorial/정확본인target·성공body/auth/contract·main 복귀는 관측됐고 DB도 전boolean true(GENERAL:0:1:0/target tutorial·updated_at 외열·장르포함11테이블·타계정session 보존/active2 revoked0)다. 그러나 browser는 reload_main, line214, failure_path=/main에서 실패했다. member_tutorial evidence는 비었고 lifecycle17requests/14finished/failed0/unfinished3/pageerror0다. account-skip과 후단admin은 없으며 complete의 최종 reload PNG도 없으므로 전체 가입/튜토리얼 성공이나25경로 완료로 세지 않는다.

판정기 결함을 좁혔다. 고정 UI 2e7c021의 authStore.partialize는 user_tutorial을 localStorage에 저장하지 않는다. App은 reload 뒤 getUserName→setUserInfo로 메모리 Zustand store만 갱신한다. 기존 wait_for_function은 localStorage.user_tutorial===1을 요구했다. 이 실제 predicate를 Node에서 실행해 persisted tutorial이 없고 메모리 tutorial1인 정상상태도 false인 RED를 재현했다. 검사를 원본 App이 재조회한 live store의 signed-in·정확target·tutorial1로 변경했다(상태쓰기·추가login·API mock 없음). 잘못된target/미완료/sign-out은 계속 false다. CI의 오류클래스는 safe JSON에 unrecognized라 특정하지 않는다. PNG3개가 fade중 어두워 tutorial 영역의 유한애니메이션 종료후 캡처하도록 보완했다. 원본UI/overlay제품코드·DB보존조건·단일mutation·lifecycle 기준은 불변이다.

앞단 password28/email20·profile/reissue/like/logout/comment3phase/search JSON은 기존 성공 범위대로 재확인했다. Password/email 전gate·DBboolean true, add33/remove37/comment각23/search22는 failed0/unfinished0/pageerror0다. User.myEmotionSummary/WIRE.NOT_FOUND·감정그래프 skeleton은 별도잔여이며 admin은 이번실행 미검증이다.

로컬: reload predicate 회귀 AssertionError RED→GREEN, 전체 evidence unittest53 OK, Python AST·embedded JS syntax·git diff --check 통과. Rust/overlay제품코드는 이번에 변경하지 않았다. 실DB·새 browser 검증은 다음 일반 Verify 전 미실행이며 webapp-testing 기준으로 기존 Linux CI만 사용한다. Clone/localDB/container/localbrowser/subagent 신규생성 없음. 관련25경로행은 유지하고, 두 account phase의 전JSON/PNG·회귀 성공 뒤에만 갱신→EditPage confirmedwithdraw로 이어간다. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지.

### 2026-10-24 Verify40076012954 비밀번호 회귀 body 수집 gate 실패

일반 Verify `40076012954`, SHA `751ad8d13483c951fbfb026ea12e8a4c0eec015d`, attempt1 completed/failure(core종료08:22:04UTC). Broad11·원본UI준비13/14는 success, 첫 실패는 step15 실HTTP/worker/MySQL이다. Safe artifact `12554215645` (`facereview-ui-browser-40076012954-1`)를 현재 Run outbox `/Users/winterholic/.ttakkari/data/outbox/f5290b4f-874f-49e9-8f1a-133da3f9fd35/verify-40076012954`에 다운로드했다. Locator는 stage=FaceReview member authentication E2E test, command=tee /tmp/aip-e2e-auth-run-"$tag".txt, exit101이다. Raw GH log/cache에는 접근하지 않았다.

Password browser는 login_new/line232/path=/my/failure_type=unrecognized다. UI6boolean·member_password6gate/verified=true 및 새password 폼/자동signed-out/새password login·reload profile PNG를 확인했다. PNG5개 중 첫 장은 fade중이고 최종 profile의 감정그래프는 skeleton이므로 그래프 성공으로 세지 않는다. 최종 모든body assertion이 실패했으며 28 API 전부 HTTP200/finished28/failed0/unfinished0/pageerror0이나 21번째 `/apply` POST `User.myEmotionSummary`만 body_captured=false다. 같은 호출의 나머지3건은 body_captured=true/ok=false/WIRE.NOT_FOUND다. Password DB는 database_status=success/전booleantrue/active1 revoked2/challenge0/mail1/flags보존/11테이블·타계정session보존이며 browser_status=failure다. Core password gate 성공을 전체 browser 성공으로 확대하지 않는다.

Collector의 실제 AST를 synthetic response로 실행한 `inspect-verify-40076012954.py`는 body읽기예외·invalid JSON 둘 다 finished1/body_captured=false/pending0, valid WIRE.NOT_FOUND JSON은 body_captured=true임을 확인했다. 함수가 finished를 먼저 기록하고 `json.loads(response.body())` 예외를 보존하지 않는 코드와 일치한다. CI에서 어느 예외가 났는지·reload와 연관됐는지는 미확인이다. safe_failure_type 문자열전달도 객체계약과 어긋나지만 기존 password/email/member scripts 불변 지시대로 수정하지 않았다. 브라우저·DB 신규실행 없이 safe artifact와 offline probe만 검사했다.

Profile/reissue browser·DBsuccess/rotation+1, like1→0·타회원보존/logoutrevoked+1, comment3phase같은ID/modified0→1→1/deleted0→0→1/무관행보존, search4gate·11테이블보존을 JSON범위 재확인했다. Add33/remove37/comment각23/search22는 failed0/unfinished0/pageerror0다. Email/account-complete/account-skip/admin 파일이 없고 Rust caller의 password browser assertion이 후단보다 먼저다. 이번에는 signup/tutorial live-store 수정의 실제 UI·DB를 확인하지 못했다. 관련25경로행·제품overlay/Rust/password/email/member/account scripts·원본UI/정책은 불변이다. 재실행·dispatch·cancel·후속예약 없이 실패기록만 갱신한다. Confirmedwithdraw, User.myEmotionSummary/graph, WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지.

### 2026-10-27 password email account body failure reason wiring

Verify40076012954(SHA 751ad8d)의 password line232 실패는 `/apply` User.myEmotionSummary 1건의 body_captured=false였고 수집기가 예외를 버려 종류 미확정이었다. password/email/account 3개 스크립트에 같은 삼킴이 공통으로 있음을 코드 대조로 확인했다(profile/admin/admin-reissue/member는 이미 기록함). 3개 `capture_body`의 except에만 `row["body_failure_reason"]` 배선(profile 패턴: `json_decode_error` vs `body_failure_reason(error)`, import 추가 포함)을 했고, 전체body gate·판정 범위·원본UI/overlay/Rust/DB보존조건은 불변이다. 계측 추가이며 gate 완화가 아니다.

Local evidence: 3종 py_compile OK, AST 배선 probe(3종 wiring+gate strict, profile 참조 일치) OK, CI 플래그 포함 evidence unittest 53 OK, `git diff --check` OK, 변경 3파일만. 로컬 liveDB/browser 미실행. 로컬 main 커밋 `85d4fcf` 완료, push는 403(daeseon-dev 권한 없음)으로 차단·원격 fa9afef 유지. 쓰기 권한 세션이 push+원격 SHA 대조 후 일반 Verify를 확인한다. 새 artifact의 failure_reason 값·실제 예외 종류·tutorial/account/confirmedwithdraw/admin 후단은 미검증이다. 관련25경로행 유지. WBS90/25·25경로전수미완·model.h5권리/visitor보류·myEmotionSummary/graph 별도잔여 유지.

### 2026-10-27 auth E2E enable_writes and emotion summary positive verification

Root cause of the `User.myEmotionSummary` WIRE.NOT_FOUND refusals (4 in password, 3 in email artifacts) is a deployment-config omission, not a product defect. A `/apply` write-extension request without `workers.enable_writes` is refused by the explicit-config gate (`spike-v6-transport/src/lib.rs:1438-1440`) as HTTP200 ok:false WIRE.NOT_FOUND. The member-auth E2E was the only facereview live deployment without the flag; trending/admin/watch/external_effects all set it, and the same definition+extension succeeds in the trending CI. One-line fix: `"enable_writes": true` in the auth `service.json` workers config. Fingerprint is unchanged (jobs already fingerprint with all extensions, `mysql_service.rs:528-534`), and all four write-db implementation modules the startup validation requires (slice14e_mypage, slice14k_watch, slice14l_frame, slice14g_request) are already written to the worker dir.

Write side effects were narrowed before enabling. The extension creates the member row on the first served call and commits exactly once (lock_version 0→1); with no watch sessions in the fixture, later calls return the base without rewriting. `user_emotion_summary` is one of the 11 snapshotted business tables, so the snapshot helper now excludes only the journey member's row (other members' rows stay on both sides), and the exclusion is covered by positive assertions instead of being dropped: password DB requires exactly one member row, all five totals zero, lock_version 1, checkpoint timestamp set with the empty-session marker; email DB requires that same row still exactly one (no duplicate, no rewrite). Profile/reissue/like/comment/search/logout/account snapshot comparisons were checked call-site by call-site and do not break (only profile/logout visit /my and write nothing after the checkpoint; search/signup snapshots contain only pre-existing rows). Browser evidence gains an `emotion_summary_served` gate (at least one tagged call ok) in the password/email verdicts. The zero totals are the original `get_emotion_summary` expectation for a member with no sessions (proven equal in 19 trending oracle cases), not faked values.

Local evidence: evidence unittest 54 OK with CI flags (one new gate test), 5-file py_compile OK, `cargo fmt --check` OK, `cargo test -p aip-cli --features mysql --test mysql_facereview_auth --no-run --locked --offline` 19.74s OK, `cargo clippy -p aip-cli --features mysql --all-targets -- -Dwarnings` 43.68s exit0, `git diff --check` OK. Build cache target/ (3.1G) removed and reclaimed after verification. No local live DB/browser runs. Committed to local main; push again blocked with 403 (daeseon-dev), remote stays fa9afef, no retry in this environment. A session with write access continues from `git push origin HEAD:main` plus remote SHA comparison. New-CI myEmotionSummary ok/bodies, graph PNGs, body_failure_reason values, tutorial live-store, account 2phase, confirmedwithdraw, admin backend stay unverified. 25-route rows unchanged. WBS90/25, full-25-routes incomplete, model.h5 rights/visitor deferrals kept.

### 2026-10-27 write-deployment enable_writes regression guard

Locks in the 48926c5 root cause as a test instead of a lesson. Audited all 8 `mysql_*.rs` extension deployments and all 7 `facereview-ui-*-browser.py` journeys by their call sites: write-extension /apply calls (inline `"request": {"extension"}` bodies, or browser journeys tagging `row["write_extension"]`) exist only in auth (via password/email journeys), trending_job, watch, external_effects, and worker_write_e2e — all five now set `enable_writes: true`. The three flagless deployments (recommend, pool_job, category_fill_job) post only to read `/extension` or run jobs, so no flag is needed and none was added. Plain-transition /apply calls and read /extension goldens are explicit negatives in the rule. `mysql_worker_write_e2e.rs` builds its write request in an `apply` helper the markers cannot see, so it is pinned by name with that limitation documented.

New `product/tests/test_facereview_write_deployments.py`: 6 rule unit tests plus a repo audit over every `mysql_*.rs` (fails listing any write-serving deployment without the flag) plus a simulation that strips the flag from the real auth source in memory and requires the guard to report it missing. No product, gate, or deployment code changed.

Local evidence: evidence unittest 62 OK with CI flags (54 existing + 8 new), 6-file py_compile OK, `cargo fmt --check` OK, auth `--no-run --locked --offline` 28.74s OK, `cargo clippy -p aip-cli --features mysql --all-targets -- -Dwarnings` 35.73s exit0, `git diff --check` OK. Cargo ran with PG* unset and shape-only dummy DSNs (build script validates shape, never connects). Build cache target/ (3.0G) removed and reclaimed after verification. No local live DB/browser runs. Committed to local main; push stays blocked in this environment (`gh auth status`: not logged in; keychain identity previously denied 403 to daeseon-dev), remote stays fa9afef, no same-denial retry. A session with write access continues from `git push origin HEAD:main` plus remote SHA comparison. CI outcome for the stacked local commits, new-artifact myEmotionSummary ok/bodies, graph PNGs, body_failure_reason values, tutorial live-store, account 2phase, confirmedwithdraw, admin backend stay unverified. 25-route rows unchanged. WBS90/25, full-25-routes incomplete, model.h5 rights/visitor deferrals kept.
