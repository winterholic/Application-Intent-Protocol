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

실행 증거는 [facereview-api-baseline.json](facereview-api-baseline.json), 패키지 버전은 [baseline-api.lock](baseline-api.lock).
공통 source 범위는 [facereview-inventory.json](facereview-inventory.json)이다.
앞으로 브라우저/실제 ML/worker/provider까지 실행해야 WBS 7.1.2 전체 환경 완료를 판정할 수 있다.
