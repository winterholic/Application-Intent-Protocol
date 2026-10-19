# Infisical 포팅 실DB 런북 (WBS 7.3.3.9)

격리 PostgreSQL 하나만 있으면 이 문서 그대로 실행된다. 같은 절차가 `run-baseline.sh` 한 파일에 들어 있고, 이 문서는 각 단계의 이유와 기대 출력을 설명한다. 마지막 실행 기록은 `porting/infisical/evidence/baseline-*.txt`다.

## 금지

- 공유 PostgreSQL(5432)과 운영 endpoint에 연결하지 않는다. 스크립트와 테스트 하네스는 loopback이 아니거나 포트가 5432이면 시작 전에 거부한다.
- `.env`와 비밀 저장소를 읽지 않는다. DB 접속 정보는 환경 변수 `AIP_TEST_DATABASE_URL` 하나로만 준다.
- 원본 Infisical 백엔드는 기동하지 않는다. 이 런북은 AIP로 옮긴 정의(`app.aip`)만 실행한다.

## 준비

1. 전용 컨테이너. 포트는 도커가 비어 있는 것을 고르게 하고 그 값을 읽는다(고정 포트를 쓰지 않는다).

   ```sh
   docker run -d --name aip-infisical-pg -e POSTGRES_USER=fixture -e POSTGRES_PASSWORD=fixture_pw \
     -e POSTGRES_DB=aip_infisical -p 127.0.0.1::5432 postgres:17
   PORT="$(docker port aip-infisical-pg 5432/tcp | head -1 | sed 's/.*://')"
   export AIP_TEST_DATABASE_URL="postgresql://fixture:fixture_pw@127.0.0.1:${PORT}/aip_infisical"
   ```

   `psql`이 호스트에 있어야 한다(seed, 정리에 사용). 다른 세션의 컨테이너를 재사용하지 말고 이름을 겹치지 않게 한다.
2. 빌드: `cargo build -p aip-cli` (기본 빌드면 된다. PoC runtime feature는 필요 없다).
3. Python 환경. worker 런타임과 SDK, JWT 서명에 쓴다.

   ```sh
   python3 -m venv /tmp/infisical-venv
   /tmp/infisical-venv/bin/pip install joserfc ./product/python
   export INFISICAL_PYTHON=/tmp/infisical-venv/bin/python
   ```

   `joserfc`는 JWKS 생성과 토큰 서명(`tools/jwks.py`), worker의 JWT 검증 spike가 쓴다. `product/python`은 공식 Python SDK(`aip_sdk`)다.

## 한 번에 실행

```sh
bash porting/infisical/run-baseline.sh
```

환경 변수: `AIP_BIN`(기본 `target/debug/aip`), `INFISICAL_WORK`(작업 디렉터리, 기본 임시), `INFISICAL_EVIDENCE`(기본 `porting/infisical/evidence`), `INFISICAL_SCHEMA`(기본 `aip_infisical_baseline`), `INFISICAL_KEEP_SCHEMA=1`(끝나고 schema를 지우지 않음).

## 단계

| 단계 | 명령 | 기대 |
|---|---|---|
| 1 | `aip service check app.aip`, `aip service gen app.aip --wire decimal --out ...` | `ok:true`. gen은 계약 지문을 가진 binding을 쓴다 |
| 2 | `python3 tools/jwks.py keygen WORK` | `WORK/jwks.json`(공개키)과 `WORK/signing-key.json`(개인키, 0600, 저장소 밖). 시험용 키이며 실제 IdP가 아니다 |
| 3 | `service.json` 작성 | `schema`, `database_url_env`, `listen: 127.0.0.1:0`, `wire: decimal`, `auth`(issuer, audience, 파일 JWKS), **`workers`**(디렉터리 `porting/infisical/workers`, `language: python`, `runtime_executable`) |
| 4 | `aip service init app.aip --config service.json` | `ok:true`와 digest 3개. 새 schema를 만든다 |
| 5 | `aip service migrate app.aip --config service.json` | 읽기 전용 계획. 정의가 같으면 `changes: []`, `blocked:false` |
| 6 | `seed.sql`의 `:schema`를 치환해 `psql -f` | users 6, folders 3 등. 사용자는 IdP가 만드는 행이라 운영자가 넣고 id를 고정한다. 명시 id는 identity 시퀀스를 올리지 않으므로 seed가 `setval`한다 |
| 7 | `aip service principal --config service.json bind --subject <sub> --actor <id>` | 토큰의 `sub`를 actor로 연결. 연결이 없으면 `WIRE.UNAUTHENTICATED` |
| 8 | `aip service serve app.aip --config service.json` | stdout 첫 줄이 `{"event":"listening","address":...,"fingerprint":...}`. `/ready`가 200 |
| 9 | `python3 tools/baseline_client.py BASE FINGERPRINT WORK` | SDK로 읽기, 쓰기, viewer 거부, job, 물리 삭제와 이름 재사용 10개 점검이 모두 `pass: true` |
| 10 | `aip service jobs --config ... list`, `aip service audit --config ... list` | job `SUCCEEDED`, 감사 행(`via: job`, `via: apply`) |

토큰은 `tools/jwks.py token WORK SUBJECT`로 직접 만들 수도 있다. 수명은 기본 900초이고 900초를 넘기면 도구가 거부한다(제품 프로필이 `exp - iat <= 900`을 요구한다).

## 시험

```sh
cd porting/infisical/tests
python3 -m unittest test_live_db -v        # 29건. AIP_TEST_DATABASE_URL이 없으면 skip
python3 mutation_check.py                  # 규칙을 하나씩 풀면 해당 시험이 실패하는지 확인
python3 -m unittest legacy_repro -v        # 수정 전 정의(53b1537)의 결함 10건 재현. 모두 FAIL이 정상
python3 -m unittest test_identity_jwt -v   # DB 없이 7건
```

`test_live_db`는 실행마다 고유한 schema를 만들고 끝나면 지운다. `legacy_repro`는 수정 전 정의가 가진 결함을 보존한 회귀 재현이라 현재 정의에서는 쓰지 않는다.

## 이 기준선이 증명하는 것과 증명하지 않는 것

증명하는 것(실제 PostgreSQL 17, 이 정의, 기본 빌드 binary):
- 조직·프로젝트·폴더·시크릿의 읽기 정책과 쓰기 권한(admin, member, viewer, no-access, 조직 admin 암묵 권한, 다른 조직 격리).
- 루트 폴더에서도 시크릿 이름 유일(다른 폴더는 같은 이름 허용), 물리 삭제 뒤 같은 이름 재생성, 일괄 삭제, 삭제 감사.
- 폴더 트리: 같은 project·env의 부모 아래에서만 생성, 부모 아래 이름 유일, 빈 폴더 삭제, 내용이 있으면 삭제 거부, `forceDelete`가 폴더와 시크릿 1000개까지 지움(하위 폴더가 있으면 거부), 루트 삭제 거부.
- 조직 생성자의 첫 admin 좌석 확보, 사용자 목록의 조직 범위 가시성.
- 서버 job 1종(`secretReminder`)의 enqueue 권한 검사, 실행, outbox 알림, 감사 행.

증명하지 않는 것:
- **비밀 값 저장·조회·회전**: 제품 방언이 `encrypted`를 거부하는 갭(G2) 때문에 정의에 없다.
- 나머지 MIT BullMQ 큐: 선언도 없다(`plan-docs/porting/INFISICAL-JOBS.md`의 "선언만" 표).
- 실제 메일·Slack 발송: job은 outbox 알림 기록까지다. 전송 receipt가 아니다.
- 실제 IdP: 토큰은 시험용 키로 서명한 것이다.
- 원본 Infisical과의 동작 동등성: 원본은 실행하지 않았다.
- worker 격리 수준(macOS 기본 `auto`)의 보안 검증과 부하·장애 시험.

## 정리

```sh
docker rm -f aip-infisical-pg
```
