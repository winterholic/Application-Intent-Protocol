# AIP 로컬 개발 bundle

이 산출물은 현재 macOS host용 **private 로컬 개발 실험**입니다. 제품 배포본이나 운영 보안 경계가 아닙니다. 다른 OS·CPU, 최종 Id/ABI, 운영 로그인, 확장 격리, release 정책은 이 bundle에서 결정하지 않습니다.

이 버전의 init은 실제 DB 구조 지문을 marker에 저장합니다. serve는 컬럼·제약·인덱스·시퀀스 정의와 객체 종류 등의 변경을 기동 전에 거부합니다. 이전 marker-only schema는 SCHEMA_NOT_READY로 거부하며 데이터·구조를 자동 변경하지 않습니다. 일반 migration과 PostgreSQL 버전 간 지문 호환은 아직 제공하지 않습니다.

DB 연결과 실행 전 시각 조회는 각각5초, init/serve의 SQL stage도 별도5초입니다. HTTP 읽기·쓰기 operation은 선언된 기한을 적용하고 status 조회는5초입니다. 커밋 응답이 불명확하면 COMMIT_UNKNOWN 또는 INIT_UNSETTLED로 보존합니다. HTTP 전체가 하나의5초 기한이라는 뜻은 아닙니다.

유효한 세션으로 시작한 WRITE가 커밋됐다면 실행 중 세션 만료를 실패 응답으로 바꾸지 않습니다. 다음 요청은 만료 토큰을 거부하며 읽기 응답·캐시에는 남은 세션 수명을 적용합니다.

`sourceSha256`는 manifest에 열거된 로컬 입력 파일의 체크섬입니다. 외부 registry cache의 내용이나 Cargo/npm 의존성을 증명하지 않으며, bit-for-bit 재현성을 보장하지 않습니다.

## 들어 있는 파일

- `bin/aip-prototype`: Cargo로 빌드한 CLI. Node/Python worker bootstrap을 바이너리에 포함하므로 checkout의 worker 경로에 의존하지 않습니다.
- `examples/app.aip`: `prototype/example/app.aip`의 정본 예제.
- `extensions/node/event.mjs`, `extensions/python/event.py`, `examples/event-seed.sql`: 정본 Event.confirm WRITE 확장과 명시적 개발 seed.
- `examples/recruitment.aip`: Recruitment READ 확장 예제와 계약.
- `extensions/node/recruitment.mjs`, `extensions/python/recruitment.py`: 예제에서 선언한 `Recruitment.stats` 구현 파일.
- `examples/recruitment-seed.sql`: 개발자가 선택한 schema에 넣을 소량의 로컬 데모 행. bundle이나 `init`은 seed를 자동 실행하지 않습니다.
- `sdk/aip-prototype-sdk-0.0.0.tgz`: 기존 JS SDK를 컴파일해 만든 offline npm tarball.
- `manifest.json`: host platform/architecture, build profile, checkout 밖에서 다시 실행할 상대 명령, source input과 bundle 파일의 SHA-256 목록. `files` 목록은 manifest 자신을 제외합니다.

## 요구 도구

Bundle을 만들 때는 저장소 checkout, Cargo 의존성의 offline cache, Node/npm, checkout의 로컬 TypeScript 컴파일러가 필요합니다. 이 bundle을 사용할 때는 macOS, Node, Python 3.11 이상, PostgreSQL과 `psql`이 필요합니다. 사용 단계에는 Cargo·AIP checkout·네트워크 설치가 필요하지 않습니다. Worker 네트워크 차단은 현재 macOS `sandbox-exec` 로컬 실험입니다.

## bundle 다시 만들기

AIP 저장소 checkout의 `prototype/` 디렉터리에서 아래 명령으로 fresh 출력 경로를 지정합니다. 출력 경로는 기존 파일·디렉터리·symlink를 덮어쓰지 않습니다. profile은 `dev` 또는 `release`입니다.

```sh
python3 tools/build-bundle.py --out <새-출력-경로> --profile dev
```

`manifest.json`의 `rebuildCommand`는 출력 경로만 바꿔 재현할 명령을 기록합니다. 산출물 SHA가 일치하는지 `shasum -a 256`과 manifest 목록으로 확인할 수 있습니다. 빌드가 실패하면 이 실행에서 새로 만든 출력 디렉터리만 제거합니다.

## Event WRITE 흐름 실행

`Event.confirm`은 공개 `Event.mark` 전이를 호출하고 결과를 돌려줍니다. 실행하려면 명시적 WRITE 설정이 필요합니다. 표준 apply와 같은 멱등 key·상태 조회·SDK pending·읽기 캐시 무효화 경로를 사용합니다.

```sh
schema=aip_event_demo
./bin/aip-prototype gen examples/app.aip --wire decimal --out /tmp/event.contract.ts --sdk-import @aip/prototype-sdk
./bin/aip-prototype init examples/app.aip --schema "$schema" --db-url 'host=localhost dbname=postgres'
psql 'host=localhost dbname=postgres' -v ON_ERROR_STOP=1 --single-transaction -v schema="$schema" -f examples/event-seed.sql
./bin/aip-prototype serve examples/app.aip --schema "$schema" --db-url 'host=localhost dbname=postgres' --listen 127.0.0.1:8080 --wire decimal --dev-actor 1 --worker-dir extensions/node --worker-lang node --enable-write-extensions
```

생성 binding과 설치한 SDK에서 서버 readiness의 `devToken`으로 호출합니다.

```ts
import {connect} from '@aip/prototype-sdk';
import {contract} from './event.contract.ts';
const client = connect('http://127.0.0.1:8080', devToken, contract);
const result = await client.writeExtension('Event.confirm', {id:'11'}, {key:'event-confirm-11'});
if (result.ok) console.log(result.output.id, result.output.count);
```

동일 key/request 재시도는 저장한 결과를 재생합니다. 새 key는 새 의도이며, 이미 checked인 행은 repeat unchanged에 따라 count 0입니다. 최종 WRITE API·격리·운영 인증은 이 private 예제에서 확정하지 않습니다.

## Recruitment 흐름 실행

Bundle 루트에서 실행합니다. 예제 schema는 비어 있어야 하며 `init`은 테이블만 만듭니다. seed는 사용자가 명시적으로 실행한 뒤에만 개발 actor 토큰 발급이 가능합니다.

```sh
schema=aip_bundle_demo
./bin/aip-prototype check examples/app.aip
./bin/aip-prototype gen examples/recruitment.aip --wire decimal --out /tmp/recruitment.contract.ts --sdk-import @aip/prototype-sdk
./bin/aip-prototype init examples/recruitment.aip --schema "$schema" --db-url 'host=localhost dbname=postgres'
psql 'host=localhost dbname=postgres' -v ON_ERROR_STOP=1 --single-transaction -v schema="$schema" -f examples/recruitment-seed.sql
./bin/aip-prototype serve examples/recruitment.aip --schema "$schema" --db-url 'host=localhost dbname=postgres' --listen 127.0.0.1:8080 --wire decimal --dev-actor 1 --worker-dir extensions/node --worker-lang node
```

Python worker를 확인하려면 마지막 명령의 `extensions/node --worker-lang node`를 `extensions/python --worker-lang python`으로 바꿉니다. 두 server는 같은 fixture의 `Recruitment.stats`를 호출합니다. 서버는 loopback에만 bind하며 로그인을 제공하지 않습니다. `--dev-actor 1`은 seed에 이미 있는 Member 1의 단기 로컬 토큰입니다.

생성 binding을 별도 소비자에서 쓰려면 SDK를 offline 설치합니다. `bundle-root`를 실제 bundle 절대 경로로 정하고 소비자 디렉터리를 새로 만드세요.

```sh
bundle_root=/absolute/path/to/bundle # 실제 bundle 경로로 바꾸세요
consumer_dir="$(mktemp -d /tmp/aip-bundle-consumer.XXXXXX)"
cd "$consumer_dir"
npm init -y
npm install --offline --ignore-scripts --no-audit --no-fund "$bundle_root/sdk/aip-prototype-sdk-0.0.0.tgz"
```

생성된 binding은 `@aip/prototype-sdk`를 import합니다. TypeScript 컴파일러와 프론트 런타임은 bundle에 넣지 않았으며 소비자 프로젝트가 정합니다.

로컬에서 더 이상 쓰지 않을 때는 먼저 서버를 `Ctrl-C`로 종료하고, 본인이 만든 schema만 직접 정리합니다. 예제는 기존 schema를 자동 삭제하지 않습니다.
