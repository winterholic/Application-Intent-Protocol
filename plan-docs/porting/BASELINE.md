# 원본 격리 API 기준선 — WBS 7.1.2 진행 중

전체 환경 작업은 아직 완료하지 않았다. 이 단계는 원본 Flask handler와 실제 MySQL/Mongo/Redis의
API 기준선이다. 브라우저·카메라·ML·Celery worker·YouTube/메일 provider는 실행하지 않았다.
Flask test client를 써서 요청하며, AIP 서버나 SDK를 통과한 결과로 계산하지 않는다.

## 재현

Docker Engine/Compose, Python 3.11과 uv를 준비한다. 아래는 예시 경로다.
원본 source는 Git commit에서만 꺼내고 `.env`를 복사하지 않는다. 운영 DB/서버에 연결하지 않는다.

```sh
mkdir -p /tmp/aip-facereview-baseline
git -C /path/to/facereview/new-facereview archive 7948b7e721837fdb73c427cb54e12cad4ec2d5af app common tests requirements.txt | tar -x -C /tmp/aip-facereview-baseline
uv venv --python 3.11 /tmp/aip-facereview-baseline-venv
uv pip install --python /tmp/aip-facereview-baseline-venv/bin/python -r plan-docs/porting/baseline-api.lock
docker compose -f plan-docs/porting/baseline.compose.yaml up -d --wait --wait-timeout 180
/tmp/aip-facereview-baseline-venv/bin/python tools/facereview_baseline.py --source /tmp/aip-facereview-baseline --inventory plan-docs/porting/facereview-inventory.json --output /tmp/facereview-api-baseline.json
docker compose -f plan-docs/porting/baseline.compose.yaml down --volumes
```

재현 시 임시 source와 DB는 이 시험 전용으로 사용한다. source hash 불일치·기존 DB 행이 있으면 runner가 거부한다.

Compose가 생성하는 DB는 tmpfs다. `down`은 이 프로젝트의 임시 DB를 없애며 다른 앱을 건드리지 않는다.
DB 계정/JWT key/비밀번호는 이 파일 묶음에만 쓰는 **합성 fixture 값**이다. 실제 credential이 아니다.
PyMySQL의 MySQL8 caching_sha2_password 연결에는 `rsa` extra(cryptography)가 필요했다.
ML 의존성은 API baseline lock에 넣지 않았다. `preload_emotion_model=False`이며 감정 결과를 mock으로 만들지 않는다.

## 입력·판정·기록

- 고정 UUID의 GENERAL/SUPER_ADMIN 사용자 두 명, 고정 영상 한 개를 원본 모델로 seed한다.
- seed의 비밀번호 hash는 원본 AuthService와 같은 Werkzeug를 쓴다. 실제 hash/토큰은 결과에 쓰지 않는다.
- 앱 factory를 먼저 시작하고 모델을 import한다. 원본 `run.py`와 import 순서를 맞춰,
  harness가 초기화 전 Redis 참조를 고정하는 가짜 결함을 만들지 않게 한다.
- 로그인·me·guest 거부·관리자 역할 경계·튜토리얼·영상 상세·댓글 CRUD·like/bookmark on/off/on·logout 후 access/refresh를 구분해 확인한다.
- 상태 코드와 원본 DB 행을 함께 검사한다. 세 번의 토글 뒤 각 단계의 행 수는 1/0/1이어야 한다.
- API 응답 값 전체·JWT·refresh cookie를 저장하지 않는다. 결과는 response key/status/error code와 실행 버전만 포함한다.
- 제품 HTTP 49개를 실제 `app.url_map`과 대조한다. 추가 `/openapi.json`, `/swagger`, `/redoc`는 framework 경로로 분리한다.

기준선의 원본 결함은 고쳐서 숨기지 않는다. 댓글 생성은 원본 DTO가 flush 전 ID를 캡처해 `comment_id=null`을
반환한다. runner는 이를 `original_anomalies`에 남기고 후속 수정·삭제 검사를 위해 실제 DB UUID를 읽는다.
이 oracle read는 시험 입력 준비이며 AIP 구현이나 원본 API 보정으로 세지 않는다.

원본 로그아웃은 refresh 쿠키만 삭제/blacklist하고 access token은 만료까지 유효하다. 이 access 동작은 원본 계약이다.
별도로 blacklist에 있는 **같은 refresh token**을 다시 넣으면 재발급이 200으로 허용되는 원본 보안 결함을 재현했다.
결과의 `source-revoked-refresh-replay`는 원본 관측값 200을 고정하는 oracle이며 보안 통과가 아니다.
포팅본은 replay 거부를 기대하고 의도적 차이로 기록해야 한다. 원본 복제를 이유로 이 취약 동작을 유지하지 않는다.
서로 다른 사용자 로그인은 client/cookie jar를 분리해 잘못된 refresh cookie를 로그아웃하는 harness 오류를 막는다.

## API 의미·저장 상태 대조 (7.1.3.1)

2026-10-15 후속 실행은 23개 API case와 12개 SQL/Redis 상태를 독립 [oracle](facereview-api-oracle.json)에 대조한다.
HTTP status·response keys·오류 code 외에 사용자/role·튜토리얼·영상/댓글 값·토글 응답을 선택해 검사한다.
저장 oracle은 튜토리얼 변경, guest 작성의 0행, 댓글 작성/수정/soft delete, like/bookmark의 1/0/1행, logout blacklist/cookie다.
MySQL 8.4.11·MongoDB 7.0.45·Redis 7.4.11의 새 tmpfs DB에서 23개/12개 전부 일치했고 원본 결함 2개는 유지됐다.

```sh
python3 -m unittest discover -s tools -p test_facereview_oracle.py
python3 tools/facereview_oracle.py --result /tmp/facereview-api-baseline.json --oracle plan-docs/porting/facereview-api-oracle.json
```

unit 10개는 정상 결과와 누락·중복·추가 case, 잘못된 HTTP/응답값/저장 상태, embedded expected 변조,
부분 실행·원본 SHA/환경 변경·scope 오인·민감 필드를 양방향으로 검사한다. 실패 진단은 값 대신 case/field 이름만 낸다.
로그인 응답의 token은 값 없이 `access_present` bool만 저장한다. 생성 댓글 UUID는 DELETE query의 해당 값만 `$comment`로 정규화하고
다른 API 경로·영상 UUID는 그대로 비교한다. 변동 시간은 이 API oracle의 의미값에 넣지 않으며 화면/ML 동등성으로 확대하지 않는다.

report의 `completed`는 계획된 API 실행 완료만 뜻한다. `scope=original-api`, `aip_ported=false`이며 browser/ML 미실행 표식은 유지한다.
case 누락·중복, 저장 검사 누락, source/환경 변경, 원본 route 누락은 comparator 오류와 nonzero 종료다.
원본 실행 결과로 기대값을 매번 다시 만드는 방식은 쓰지 않는다. oracle의 의미값은 고정 source의 DTO/schema·합성 seed를 기준으로 정했다.

## 실행 증거 gate (7.1.5.1)

`tools/facereview_evidence.py --manifest <run>/manifest.json`은 result·독립 oracle·inventory·API lock·Compose를
같은 묶음에 두고 hash, 원본 SHA, 명령 exit, 정리 exit, 범위 표식과 oracle 일치를 확인한다.
API/cleanup exit는 반드시 정수 0이어야 하며, 누락/변조/부분 report·디렉터리 밖 파일·민감 command argument를 거부한다.
JSON으로 읽히더라도 null/잘못된 shape이면 실패한다. 진단은 민감 값 대신 필드 이름을 사용한다.
hash는 파일 이동 중 훼손을 확인하는 수단이며, 악의적으로 전체 묶음을 다시 작성한 사람의 증거를 인증하는 서명은 아니다.
browser/ML/AIP port는 `not-executed`다. 이 API 증거 gate를 통과해도 전체 앱 포팅/실배포 완료로 계산하지 않는다.

## 한 번 실행하는 API runner (7.1.5.2)

준비된 API venv와 Docker Engine/Compose를 사용한다. 입력 source는 고정 Git revision에서 새 임시 디렉터리로
추출한다. 기존 원본 작업 트리·`.env`를 복사하지 않고 결과 디렉터리가 있으면 덮어쓰지 않는다.
Python 실제 설치 목록과 lock·검토된 oracle의 패키지가 일치해야 DB를 시작한다.

```sh
python3 tools/facereview_runner.py --backend-repo /path/to/facereview/new-facereview --python /tmp/aip-facereview-baseline-venv/bin/python --out /tmp/new-facereview-api-run
python3 tools/facereview_evidence.py --manifest /tmp/new-facereview-api-run/manifest.json
python3 -m unittest discover -s tools -p 'test_facereview_*.py'
```

runner는 default baseline 프로젝트가 남아 있으면 실행을 거부한다. 각 새 실행은 무작위 이름의 Compose 프로젝트를
소유하고 자신의 프로젝트만 `down --volumes`로 정리한다. 원본 실행/검사 실패와 정리 실패는 nonzero로 끝난다.
컨테이너 시작이 일부 실패해도 자신의 프로젝트를 정리하며 API 실패 때에도 정리 순서를 수행한다.
명령 argv/exit를 저장하되 응답·인증 값과 subprocess stderr를 evidence에 그대로 복사하지 않는다.
archive의 절대 경로·상위 경로·symlink·hardlink·device 입력을 추출하지 않는다.

2026-10-15 실제 runner 실행은 23개 API·12개 SQL/Redis 상태·원본 결함 2개를 보존했고,
API exit 0·cleanup exit 0·evidence 오류 0·남은 해당 프로젝트 컨테이너 0이었다.
관련 unit 34개는 inventory8·oracle10·evidence9·runner7로 구성된다. CI와 브라우저/ML/worker/provider 전체 통합은
7.1.5.3에 남겨둔다. 이 runner는 원본 API 기준선 도구이며 AIP로 업무를 대체한 구현이 아니다.

실행 증거는 [facereview-api-baseline.json](facereview-api-baseline.json), 패키지 버전은 [baseline-api.lock](baseline-api.lock).
공통 source 범위는 [facereview-inventory.json](facereview-inventory.json)이다.
앞으로 브라우저/실제 ML/worker/provider까지 실행해야 WBS 7.1.2 전체 환경 완료를 판정할 수 있다.
