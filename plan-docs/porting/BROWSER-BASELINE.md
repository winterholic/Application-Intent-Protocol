# 원본 브라우저 기본 화면 기준선

## 2026-10-17 댓글 UI 실행 환경 갭

WBS 7.1.3.2.2 댓글 UI 대조를 시작하려 했으나 이번 실행에서는 브라우저가 앱까지 도달하지 못했다. 설정된 Ttakkari browser는 `http://127.0.0.1:23173/watch/00000000-0000-4000-8000-000000000010`에 `net::ERR_CONNECTION_REFUSED`를 반환했다. 앱 서버가 이번 실행에 떠 있지 않은 상태였다. 또한 지정된 Mac browser preflight는 `status=blocked`, `browser_launched=false`를 반환했고 `mach-register`/`mach-lookup`을 거부했다. 이 결과 뒤에는 다른 브라우저를 실행하지 않았다.

따라서 이전 실행에서 관찰한 YouTube 오류 모달의 원인은 이번 실행에서 확인하지 못했다. 외부 YouTube 접근 제한이 원인일 가능성은 **추정**으로 남긴다. 댓글 화면 조작, 댓글 생성·수정·soft delete UI, 저장 후 화면 반영은 미실행이고 AIP 갭 판정도 없다. 이 환경 실패를 제품 대체율에 합산하지 않는다.

다음 허용된 browser 실행의 harness 조건은 원본 프런트/API 및 seed를 변경하지 않고 YouTube 의존성만 고립하는 것이다. 우선 `/embed/` iframe과 해당 YouTube player API 요청을 명시된 로컬 fixture로 route해 player 오류가 댓글 화면을 가리지 않게 한다. 응답은 화면에서 playback 성공을 주장하지 않는 고정 stub이어야 하며, 원본 댓글 HTTP 요청은 기존 격리 API로 계속 보낸다. iframe stub이 화면을 안정적으로 열지 못하면 재현 가능한 위치의 UI 닫기 control을 실제 클릭하는 대안을 사용한다. 실행 기록에는 stubbed provider request와 미검증 playback을 남기고, 요청을 전부 차단한 결과를 댓글 UI 성공으로 계산하지 않는다. 이 조건은 원본 코드 수정 없이 브라우저 harness에만 적용한다.

2026-10-15. **원본 앱의 기본 화면 검증이며 AIP 포팅 결과가 아니다.**
사용자/관리자 로그인과 13개 정상·오류 화면을 실제 브라우저→실제 HTTP→원본 handler→MySQL/Mongo/Redis로 확인했다.
영상 제공자는 차단했다. 시청 화면은 제공자 오류 상태만 검증했으며, 재생·댓글 UI 조작·웹캠/ML·예약 업무는 남아 있다.

## 확인 범위

| 역할 | 실행한 검사 |
|---|---|
| 사용자 | 로그인, 메인, 검색, 즐겨찾기, 마이페이지, 정보수정, 시청 화면의 영상 제공자 오류 상태 |
| 관리자 | 로그인, 대시보드, 회원 목록, 영상 목록, 영상 요청, 댓글 관리 |

로그인은 입력창·버튼을 실제 조작하고 이후 화면은 원본 route로 이동해 렌더된 내용을 확인한다.
회원 정보의 합성 이름/이메일, 합성 영상 제목 등 필요한 내용을 기다린 뒤 screenshot을 찍는다.
목록이 로딩됐다는 확인을 영상 재생·감정분석·전 제품 상태·실DB 변경 동등성으로 확대하지 않는다.
전체 기능 분모는 기존 inventory이며 13개 화면 case는 그 분모를 대체하지 않는다.

`facereview_browser.py`는 원본 API fallback을 차단하는 URL 경계와 부분 실행 결과 gate를 함께 제공한다.
요청/응답은 method·origin·path·status만 저장하고 query·header·cookie·body·인증 값을 저장하지 않는다.
이 파일은 **축약 네트워크 기록이며 원본 HAR가 아니다.** 실제 UI 검증에 원본 API test_client를 사용하지 않는다.
사용자·관리자는 별도 browser context/cookie jar를 사용한다.

## 환경과 source

source SHA는 [inventory](facereview-inventory.json)에 고정된 세 저장소를 그대로 쓴다.
`facereview_ui_sources.py`는 고정 Git archive의 필요한 파일만 새 디렉터리로 복사하고 `.env`와 원본 작업 트리는 읽거나 수정하지 않는다.
원본 React/스타일/API 코드는 변경하지 않는다. 시험용 Vite 설정만 추가해 `/api`·`/socket.io`를 로컬 원본 서버로 고정한다.

- backend: Python API lock의 원본 factory, `preload_emotion_model=False`, Werkzeug 개발 HTTP 서버(loopback 23175).
- user: Yarn 4.12.0의 `--immutable`, Vite 7.3.1, loopback 23173.
- admin: `npm ci --ignore-scripts`, Vite 8.1.3, loopback 23174.
- Node: v26.9.0. 브라우저 도구 venv는 API lock venv와 분리하며 Playwright 1.64.0을 사용한다.
- 브라우저: Linux arm64 Chrome Headless Shell 156.0.8078.12, Docker image digest
  `chromedp/headless-shell@sha256:f70908c8020555a3bf89a070140913b443a03ead28c1a66563c3912a3b32b08d`.
- 한글 screenshot: 컨테이너의 `fonts-noto-cjk=1:20240730+repack1-1`. 서비스/UI 디자인 변경이 아니다.

Mac sandbox에서 native Chrome/headless shell의 MachPortRendezvousServer 기동이 Permission denied로 실패해 Linux 브라우저를 사용했다.
호스트 권한을 바꾸지 않았다. CDP는 127.0.0.1:29222에만 공개하고 브라우저는 `host.docker.internal`로 Mac의 loopback 시험 서버에 접근한다.
허용 hostname은 이 alias와 127.0.0.1뿐이며 port는 23173/23174/23175뿐이다. 공개 운영 API·YouTube/CDN 요청은 차단한다.
실제 Chrome/Safari·모바일 교차 브라우저 보증은 하지 않는다.

원본 Socket.IO는 `host.docker.internal` 시험 주소를 origin 허용 목록에서 거부했다.
이는 이번 기본 화면 실행의 환경 제약으로 기록하며, WebSocket 연결·프레임 전달이 동작했다고 판정하지 않는다.
원본 origin 설정을 임의로 완화하거나 연결 성공을 mock으로 만들지 않았다.

### 원본 Socket.IO polling 입력 거부 기준선 (2026-10-17)

브라우저 origin 차단과 분리해 허용된 `http://localhost:5173` Origin으로 격리 원본 API에 Engine.IO polling 연결을 열었다.
`watch_frame`에 빈 객체를 실제 전달한 뒤 원본 응답 `status=error`, `message=Missing required fields`를 받았고 client transport 종료를 확인했다.
고정 backend SHA는 `7948b7e721837fdb73c427cb54e12cad4ec2d5af`이며 실제 source Python 파일 hash를 실행 전에 다시 대조한다.
보고서 [facereview-socket-baseline.json](facereview-socket-baseline.json)은 `scope=original-socket`, `aip_ported=false`, `ml=not-executed`, `complete=false`를 보존한다.
프레임 bytes는 보내지 않았고 DB/Mongo 쓰기를 기대하지 않았으므로 이 실행은 유효 영상 프레임·watch timeline·ML·provider 검증이 아니다.
polling만 확인했다. Python 환경의 `websocket-client` 미설치로 WebSocket transport는 미실행이며, browser origin 차단은 별도 환경 결과다.
재실행에서 이 polling client가 유일한 Socket.IO 연결인 상태로 서버를 종료할 때 `disconnect handler error`와 `TypeError: handle_disconnect() missing 1 required positional argument: 'message'`가 출력됐다. client transport는 닫혔으나 원본 callback 처리는 실패했다. 이는 원본의 결함 관찰이며 AIP와 기능 대조 전이므로 AIP 갭으로 분류하지 않는다.

재현은 격리 baseline Compose와 고정 API source를 준비하고 기존 API baseline으로 seed한 다음 실행한다.

```sh
/tmp/aip-api-venv/bin/python tools/facereview_socket_baseline.py \
  --source /tmp/aip-ui/backend \
  --inventory plan-docs/porting/facereview-inventory.json \
  --output /tmp/facereview-socket-result.json
python3 -m unittest discover -s tools -p 'test_facereview_socket.py'
```

## 재현 명령

먼저 [API 환경](BASELINE.md)의 lock venv와 Docker를 준비한다. 다음 명령의 `/tmp` 경로는 예시다.

```sh
python3 tools/facereview_ui_sources.py --facereview-root /path/to/facereview --out /tmp/aip-ui
cd /tmp/aip-ui/user
env CI=1 YARN_GLOBAL_FOLDER=/tmp/aip-yarn-global YARN_ENABLE_GLOBAL_CACHE=false YARN_ENABLE_TELEMETRY=0 node .yarn/releases/yarn-4.12.0.cjs install --immutable
cd /tmp/aip-ui/admin
env npm_config_cache=/tmp/aip-npm-cache npm ci --ignore-scripts --no-audit --no-fund
```

AIP 저장소 위치에서 시험 전용 Compose를 시작하고 원본 API baseline으로 seed한다.

```sh
docker compose -p aip-face-browser-run -f plan-docs/porting/baseline.compose.yaml up -d --wait --wait-timeout 180
/tmp/aip-api-venv/bin/python tools/facereview_baseline.py --source /tmp/aip-ui/backend --inventory plan-docs/porting/facereview-inventory.json --output /tmp/aip-ui-api-result.json
python3 tools/facereview_oracle.py --result /tmp/aip-ui-api-result.json --oracle plan-docs/porting/facereview-api-oracle.json
```

세 프로세스를 별도 자식 프로세스로 시작하고 health/HTTP 준비를 확인한다. 종료할 때 시작한 자식만 종료한다.

```sh
/tmp/aip-api-venv/bin/python tools/facereview_http.py --source /tmp/aip-ui/backend --inventory plan-docs/porting/facereview-inventory.json
# cwd=/tmp/aip-ui/user
env VITE_SOCKET_URL=http://host.docker.internal:23173 YARN_GLOBAL_FOLDER=/tmp/aip-yarn-global node .yarn/releases/yarn-4.12.0.cjs vite --config aip-baseline.config.mjs
# cwd=/tmp/aip-ui/admin
env VITE_API_BASE_URL='' node node_modules/vite/bin/vite.js --config aip-baseline.config.mjs
```

Linux 브라우저와 별도 Playwright venv를 준비한 뒤 실행한다. apt는 컨테이너 내부의 한글 폰트 설치다.

```sh
docker run --rm -d --name aip-face-browser-cdp -p 127.0.0.1:29222:9222 chromedp/headless-shell@sha256:f70908c8020555a3bf89a070140913b443a03ead28c1a66563c3912a3b32b08d
docker exec -e DEBIAN_FRONTEND=noninteractive aip-face-browser-cdp sh -c 'apt-get update -qq && apt-get install -y -qq fonts-noto-cjk'
docker restart aip-face-browser-cdp
uv venv --python 3.11 /tmp/aip-browser-venv
uv pip install --python /tmp/aip-browser-venv/bin/python playwright==1.64.0
/tmp/aip-browser-venv/bin/python tools/facereview_browser.py --out /tmp/new-browser-result
docker stop aip-face-browser-cdp
docker compose -p aip-face-browser-run -f plan-docs/porting/baseline.compose.yaml down --volumes
```

`--out`은 기존 디렉터리를 덮어쓰지 않는다. 실패 때에도 본인이 시작한 Vite/HTTP/브라우저·Compose만 정리한다.
완전한 browser lifecycle/CI 자동화는 아직 연결하지 않았다. `facereview_runner.py`의 기존 API 자동 정리 계약을 바꾸지 않았다.

## 결과 판정

부분 UI 결과는 `scope=original-browser`, `aip_ported=false`, `ml=not-executed`, `complete=false`다.
13개 예정 case의 누락/중복/실패, 처리되지 않은 browser exception, 외부 운영 endpoint 응답,
API 5xx 또는 인증 값이 섞인 기록을 실패시킨다. 기본 화면 실행 성공과 전체 앱 포팅 gate를 구분한다.
관련 테스트는 기존 34개 + browser 경계/기록/부분 gate 3개(변조 음성 control 포함)다.
Socket.IO polling 입력 거부 gate는 별도의 `tools/test_facereview_socket.py` 네 단위 테스트를 사용한다.
browser/API/UI/ML/provider 전체 gate 완료로 합산하지 않는다.

남은 UI gate는 댓글 작성/수정/삭제·토글과 저장 결과, 잘못된 입력/권한/로그아웃·재접속 상태,
실제 영상/웹캠과 연결한 journey, 전 화면 비교 oracle 및 UI·ML·provider CI 통합이다.
