# 확장 실행 구조 정확성 검토 (r4)

기준일: 2026-10-04. 기준은 창시자 통합 지침 원칙 7·Q7, E RK-05·RK-08, C B.7이다. 작성자 주장은 미검증으로 출발했다. 저장소 산출물은 이 파일 하나다. git·.env·비밀 저장소는 읽지 않는다. 발견은 확인 즉시 append한다.

## 실행 범위와 방법

원본 테스트의 schema 삭제와 Python __pycache__ 생성을 피하려고 네 spike의 소스·fixture·테스트를 임시 디렉터리 `/private/tmp/aip-r4-3z9h6m9_`에 복사했다. 테스트 의미는 유지하고 DB schema 식별자만 `aip_r4_*`로 바꿨다. 컴파일 출력은 원본 spike의 `target/`에만 둔다. DB는 이번 실행이 생성한 schema만 사용하고 종료 시 삭제한다. 추가 실험은 복사본에만 둔다.

## 발견 (확인 즉시 추가)

### R4-01 · P3 · Q4 · sandbox-exec 격리 대안은 이 검토 환경에서 재검증 불가

문제: 기본 worker의 기대 결과는 재현되지만 sandbox-exec worker는 Node·Python 모두 DB probe와 ctx 호출에서 WORKER_FAILED다.
영향: 이번 실행으로 네트워크 차단·파일·자식 프로세스·외부 API 범위를 실증할 수 없다. 작성자의 과거 실행을 허위라고 판정하지 않는다.
조치: sandbox-exec 실행을 허용하는 환경에서 따로 검증하고, 기동 실패 원인을 서버 내부 로그에 남긴다.

근거: `spikes/spike-v4-worker/src/lib.rs:71`의 프로파일은 `(allow default)(deny network*)`, `:80`은 stderr 폐기. `tests/v4_1.rs:110`의 비교에서 양 언어가 실패했다. 직접 진단 명령 `/usr/bin/sandbox-exec -p '(version 1)(allow default)(deny network*)' /usr/bin/true` 결과는 `sandbox-exec: sandbox_apply: Operation not permitted`. 이번 Codex 실행 환경의 중첩 sandbox 적용 제한으로 추정하며 원인 설정까지 확인 못함: 환경 보안 설정 변경은 범위 밖이다.

확인 못함: 이 프로파일의 실제 파일 접근·자식 프로세스·TCP/Unix socket 차단·외부 API 차단. 정적으로는 네트워크 deny만 있고 파일/프로세스 deny는 없다. 문서 V4-R5가 파일 격리 없음을 명시한 점은 코드와 일치한다.
