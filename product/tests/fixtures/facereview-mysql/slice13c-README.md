# Slice13-C: watch/video, watch/recommended (WBS 7.2.3)

원본 `new-facereview@7948b7e721837fdb73c427cb54e12cad4ec2d5af`의 `WatchService.get_video_detail`, `get_recommended_videos`를 AIP worker extension으로 포팅한 증분이다. **전체 API 동등이 아니다.** 아래 "주장 범위"를 따른다.

## 주장 범위
| 주장 | 가능? | 근거 |
|---|---|---|
| timeline 압축 알고리즘이 원본 실함수와 같다 (임의 세션 집합 입력) | 가능 | `slice13c_oracle.py` 다중 사용자 무작위 40건 + 특이 입력 |
| `watch/video`의 timeline이 원본과 같다 | **금지** | 원본은 해당 영상의 모든 사용자 세션을 합산한다. worker는 `youtube_watching_data`에서 호출자 본인 세션만 본다(`timeline_scope = "caller"`). 서로 다른 사용자의 세션이 있는 영상은 값이 달라진다. oracle이 이 차이를 일부러 "DIFFERS"로 확인한다 |
| `watch/recommended`가 원본과 같다 | 조건부 | 로직은 원본 실함수와 일치(동점 순서 제외). 실서비스는 정의 언어에 Mongo grant 선언이 없어 현재 호출 불가(D1) |
| view_count 증가가 원본과 같다 | 부분 | 호출당 +1은 같다. 원본은 상세 조회 응답과 같은 요청에서 증가, AIP는 `recordView` 별도 호출. 아래 "view_count" 참고 |

## 파일
- `slice13c_watch.facereview.aip`: `facereview.aip` + 표시된 slice13c 블록(차이는 `reference/slice13c_definition_delta.py`가 검사). 필터 삭제, guest 허용, row policy 완화 없음.
- `extensions/slice13c_watch.py`: 순수 포팅 함수와 worker 진입점.
- `reference/slice13c_oracle.py`: 원본 실함수 oracle. `reference/slice13c_service_e2e.py`: 실 서비스 E2E. `reference/slice13c_coreprobe_e2e.py` + `slice13c-core-probe.patch`: Mongo grant를 선언할 수 있게 한 scratch 바이너리 전용 증거(제품 코드 아님).
- `slice13c-dba.sql`: `video.view_count` NOT NULL DEFAULT 0.

## 실행
oracle(원본은 `git archive 7948b7e app common`으로 추출한 읽기 전용 사본):
```sh
FACEREVIEW_SOURCE_PATH=<extracted> S13C_MYSQL_URL='mysql+pymysql://…@127.0.0.1:<고포트>/<전용 DB>?charset=utf8mb4' \
S13C_MONGO_URL='mongodb://127.0.0.1:<고포트>' S13C_ORACLE_DUMP=<out.json> <venv python> reference/slice13c_oracle.py
```
`S13C_ORACLE_DUMP`는 모든 케이스의 원본 응답과 모듈 응답을 JSON으로 남긴다. 서비스 E2E는 `AIP_BIN`, `S13C_ADMIN_URL`(전용 컨테이너 관리자 URL), `S13C_TAG`가 필요하다. 모두 loopback과 비기본 포트를 요구하고 기본값이 없다.

## 원본 특이동작 처리
모듈은 원본 산술을 그대로 옮겼고 oracle이 각 항목을 원본 실함수와 대조한다.
- duration 0/NULL은 `(duration or 1) * 100`, 즉 1초 창.
- 음수 time key: `int(cs / bucket_size)`가 음수 index가 되어 뒤에서부터 채워진다. 범위를 벗어나면 `IndexError`. 원본과 같은 실패가 나며 기본값 성공으로 숨기지 않는다.
- 숫자로 읽히지 않는 key만 있고 점수 행은 유효하면 100개 0점. 점수 행이 모두 5개 미만이면 1점 기본값. 둘은 다른 결과다.
- 점수가 `"x"`면 `ValueError`, `None`이면 `TypeError`. 원본과 같은 실패, 성공으로 숨기지 않는다.
- `round(…, 1)`은 같은 Python 이진 float 합에 대해 같은 결과(.x5 경계 포함).
- `VideoDistribution.from_dict`: 문서에 `dominant_emotion` 키가 없으면 'neutral', 명시적 null이면 None.

## Mongo 완전성
capability는 호출당 문서 100개, 응답 1 MiB를 상한으로 하고, 초과 페이지는 잘라서 주지 않고 `WIRE.BAD_VALUE`로 거절한다. `_find_all`이 안정 정렬(`_id`)과 skip으로 페이지를 나누고, 거절되면 페이지를 절반으로 줄여 다시 시도한다. 문서 하나가 한 호출에 못 담기거나 2만 건을 넘으면 `BudgetExceeded`/오류로 끝내며 부분 결과를 성공으로 돌려주지 않는다. `$in`은 100개 단위로 나눈다. Video 목록은 100행 페이지로 offset을 올리며, offset 상한(100000)을 넘으면 오류다.

## view_count
- 원본: `video.view_count += 1`을 Python에서 읽고 더한 값으로 갱신한다. 응답 DTO는 증가 전 값(예: 7)이고 DB는 8이 된다. 요청이 겹치면 증분이 사라진다(lost update, oracle에서 두 세션으로 재현).
- AIP: `watchDetail`은 증가 전 값을 돌려주고 쓰지 않는다. `recordView`가 `view_count = view_count + 1`을 같은 MySQL fenced transaction에서 audit와 함께 실행한다. E2E에서 겹친 호출 16건이 lost update 없이 정확히 합산되고, 거절된 호출은 부분 쓰기 없이 같은 키로 다시 보내면 한 번만 실행된다.
- 멱등 키는 actor 단위다. 같은 키를 다른 사용자가 보내면 새 요청이다.
- 한 행에 호출이 몰리면 일부 호출이 `WIRE.EXTENSION_ERROR`(retry never)로 거절된다. 원인(락 대기/직렬화 추정)은 미확인이다. 결과는 안전하지만 클라이언트가 재시도 가능 여부를 코드로 구분할 수 없다.

## 코어 의존성
| # | 내용 |
|---|---|
| D1 | 정의 언어에 `access mongo.<collection>`/`redis.<key>` 선언이 없다. 실서비스에서 `ACCESS_NOT_DECLARED` |
| D2 | `youtube_watching_data`는 호출자 본인 행만. 전사용자 timeline 합산에는 개인정보 없는 서버 집계 계약이 필요 |
| D3 | private 필터가 숫자 `actor_id`(i64)로 비교하지만 FaceReview의 `user_id`는 UUID 문자열. `session.actor_resource_id`의 신뢰된 UUID를 `CapabilityPrincipal`에 전달해야 한다(별도 core worker 담당). 지금은 UUID principal이 본인 세션도 못 본다 |
| D4 | extension 입출력은 스칼라만. timeline과 추천 목록은 JSON 문자열(`timeline_data_json`, `videos_json`) |
| D5 | extension은 서명 세션 필수. 원본의 `login_optional` guest는 처리 못 한다 |
| D6 | Python worker의 `asyncio.StreamReader` 기본 64 KiB 줄 제한이 capability의 1 MiB 상한보다 작아, 약 75 KB 이상의 세션 문서를 읽으면 `WIRE.WORKER_FAILED`(약 15만 바이트는 `EXTENSION_ERROR`)로 실패한다. 원본 key는 0.5초 간격이라 약 12분 영상부터 해당. 조용한 절단은 아니고 명시적 실패 |
| D7 | `view_count`가 live 컬럼은 nullable이나 언어는 non-null Int만 증감. DBA 단계 필요 |

## 서버 집계 계약 제안 (설계안, 사용자 확정 아님)
timeline에 필요한 입력은 key별 5감정 합과 세션 수뿐이다. 서버가 `youtube_watching_data`에서 `video_id` 한 건에 대해 `{time_key: {sums: [5], count}}`만 집계해 돌려주고 user/session 식별자는 내보내지 않으면 인가 완화 없이 원본과 같은 입력을 만들 수 있다. 모듈의 `compress_sums(emotion_sums, counts_per_key, duration)`가 이미 이 입력을 받는다. 주의: 서버 쪽 합산 순서가 원본(문서 순서)과 다르면 float 합이 달라져 `round(…, 1)` 경계에서 1ulp 차이가 날 수 있으므로 oracle로 다시 대조해야 한다.
