# 호환 표

엔진(Core)과 SDK의 버전, 그 버전이 말하는 protocol과 형식의 대응이다. 릴리스마다 한 줄을 더한다. `scripts/check-versions.mjs`가 현재 버전의 행이 있고 protocol과 Core IR 값이 코드와 같은지 검사한다.

## 읽는 법

- **엔진·SDK 버전**: Cargo workspace 버전이 원천이다. Core(`aip`), TypeScript SDK, Python SDK는 같은 숫자를 쓴다(lockstep). Python의 사전 릴리스 표기만 PEP 440(`1.0.0rc1`)으로 다르다.
- **protocol**: `crates/aip-contract`의 `PROTOCOL_VERSION`. 계약 문서(`GET /aip/describe`)의 `protocol` 필드와 생성 binding 헤더 주석에 실린다. **major가 호환 경계다.** 같은 major 안에서는 계약 문서에 선택 필드를 더하는 변경만 허용한다. 필드 제거, 의미 변경, 오류 코드 삭제는 major를 올린다. 오류 코드 레지스트리(`spec/wire-errors.md`)는 추가가 minor, 삭제가 major다.
- **Core IR**: `crates/aip-ir`의 `CORE_IR_VERSION`. 정의가 의미 분석을 거쳐 만들어지는 Core IR의 직렬화 태그다. 배포 저널에 저장되므로 protocol과 따로 움직인다(아래 "올리지 않은 표지").
- **앱 매니페스트**: `aip.app.json`의 `$schema`. 형식 번호는 정수로 독립 증가한다.
- **패키지 형식**: 앱 패키지 `BUILD.json`의 `format`.
- **계약 지문**: `x-aip-contract`와 `/capabilities`의 `contractFingerprint`. 앱 단위 정체성이며 SemVer와 독립이다. 배포가 바뀌면 binding을 다시 생성해야 한다.

## 버전 대응

| 엔진·SDK | protocol | Core IR | 앱 매니페스트 | 패키지 형식 | 비고 |
| --- | --- | --- | --- | --- | --- |
| 1.0.0 | `aip-protocol/1.0` | `aip-core/0.11` | `app-manifest-v1.json` | 1 | 최초 안정 릴리스(초안, 소유자 승인 전) |
| 0.1.0 | `aip-protocol/0.1` | `aip-core/0.11` | `app-manifest-v1.json` | 1 | 사전 릴리스. 공개 배포 없음 |

## 1.0에서 바꾼 표지와 그대로 둔 표지

| 표지 | 1.0 | 이유 |
| --- | --- | --- |
| protocol | `aip-protocol/0.1` → **`aip-protocol/1.0`** | 0.x는 "깨질 수 있음"으로 읽히므로 1.0 선언과 맞지 않는다. 계약 문서의 `protocol` 필드와 생성 binding 헤더 주석만 바뀐다. 계약 지문은 이 문자열에서 만들어지지 않으므로 이미 배포된 앱의 지문은 그대로다. 골든 계약 4개, 골든 client 4개, 예제 binding 4개의 변경은 각각 이 한 줄뿐이다 |
| Core IR | `aip-core/0.11` **유지** | 이 태그는 Core IR의 canonical JSON에 들어가 `digest()`에 반영되고, `aip-runtime`이 `_aip_deployment`의 `digest`와 `ir` 열에 저장한다. 문자열을 바꾸면 기존 배포 전부의 digest가 달라져 "변경됨"으로 읽히고 계획을 다시 세운다. 더 나쁘게는 롤백한 옛 바이너리가 새 태그를 `POC.I101`로 거부한다. 릴리스 gate의 롤백 경로가 이 위험을 직접 건드린다. IR의 의미가 실제로 바뀔 때만 올린다 |
| 앱 매니페스트 `$schema` | `app-manifest-v1.json` 유지 | 형식이 바뀌지 않았다 |
| 패키지 형식 `format` | `1` 유지 | 형식이 바뀌지 않았다. 패키지 안의 SDK 파일 이름만 버전·이름 설정에서 파생된다 |
| 메시지 protocol(subscribe) | 유지 | 계약 문서 버전과 별개다(`crates/aip-runtime/src/subscribe.rs`) |

SDK가 서버 protocol을 비교하는 검사(`/capabilities`에 `protocol`을 싣고 클라이언트가 지원 major와 대조해 `CLIENT.PROTOCOL_ERROR`를 내는 방식)는 이번 릴리스에 넣지 않았다. 현재 SDK는 계약 지문 불일치만 검사한다. npm `package.json`의 `aip.protocol`과 Python `aip_sdk.PROTOCOL`은 이 SDK가 말하는 protocol을 표시하며, 빌드 때 `PROTOCOL_VERSION`과 일치하는지 검사한다. 런타임 비교는 `TBD`다.

## SDK 버전 식별

- **노출**: TypeScript는 `SDK_VERSION`, `SDK_IDENTITY`(`typescript/<version>`), `SDK_PROTOCOL`을, Python은 `aip_sdk.__version__`, `SDK_IDENTITY`(`python/<version>`), `PROTOCOL`을 공개한다. 값은 Cargo workspace 버전과 `PROTOCOL_VERSION`에서 파생하며 `scripts/check-versions.mjs`가 어긋나면 실패한다.
- **전송**: 두 SDK는 모든 HTTP 요청에 `x-aip-sdk: <identity>` 헤더를 보낸다.
- **서버**: 엔진은 이 헤더를 요청 로그 줄(`"sdk":"typescript/1.0.0"`)과 OTLP span 속성(`aip.sdk`)에만 쓴다. 48자 이하의 안전한 문자(`A-Za-z0-9._/+-`)가 아니면 무시하고, 값이 있든 없든 응답과 거부 판단은 달라지지 않는다. 버전 협상이나 거부는 없다. 호환 신호는 여전히 계약 지문이다.
- **옛 SDK의 증상**: 이 헤더가 생기기 전의 SDK(개발 빌드 `0.1.0`은 날짜가 달라도 모두 같은 번호였다)는 헤더를 보내지 않고, 로그에 `sdk`가 없다. 새 binding과 옛 SDK를 섞으면 SDK에 없는 기능을 부르는 순간 `TypeError`(예: `client.session is not a function`)로 끝난다. 서버 응답 문제가 아니므로 로그에서 `sdk` 필드가 없는 호출자를 찾아 SDK를 binding과 같은 릴리스로 올린다.
- **브라우저**: 새 SDK가 보내는 `x-aip-sdk`는 CORS preflight에서 허용 목록에 있어야 한다. 이 변경 전의 엔진(`0.1.0` 개발 빌드)은 preflight를 `WIRE.BAD_REQUEST`로 거절하므로, 브라우저에서 SDK 1.0.0을 쓰려면 엔진도 1.0.0 이상이어야 한다. 서버 간 호출(Node, Python)은 CORS가 없어 영향이 없다.

## 지원 플랫폼

CLI 아카이브의 target과 최소 요구는 [INSTALL](product/INSTALL.md#지원-플랫폼과-최소-요구)을 따른다.
