# AIP 1.0 표현력 감사와 P0 설계

- 기준: `origin/main` `4e1325644b267f2b3177a327258c0882ee5711c2`(2026-11-21 03:14 +0900, "docs: 1.0 지원 범위·호환 계약·표현력 한계…").
- 브랜치: `design/expressiveness-1.0-20261121`. 이 디렉터리 외 파일은 바꾸지 않았다. 제품 코드 변경 없음.
- 판정 경로: 제품 `aip service`(`.aip`). Core `.aipoc` 기능은 합산하지 않았다.
- 실행 환경: macOS arm64(18코어), rustc 1.98.1, PostgreSQL 17.11(전용 컨테이너 `aip-expr-20261121-pg`, 127.0.0.1:56877), 임시 fixture `/tmp/aip-expr-fx`, 임시 앱 `/tmp/aip-expr-run`.

| 파일 | 내용 |
| --- | --- |
| [audit.md](audit.md) | 실무 시나리오 20개 A~E 판정, E1~E7 재검증, 발견한 불일치 |
| [decisions.md](decisions.md) | P0-A~D 결정표, 공통 의미 계약, 공개 count 소유자 결정 항목, P1 분류 |
| [test-plan.md](test-plan.md) | 성공·실패 fixture 목록과 실행 명령 |
| [work-packages.md](work-packages.md) | 수직 구현 묶음, 파일 충돌, 순서, 병렬 가능 여부 |
| [build-baseline.md](build-baseline.md) | cargo 빌드·테스트 비용 측정값과 target 권장안 |

구현 순서 요약: **WP1(P0-B 산술) → WP2(P0-A 단계 참조)** 를 한 작업자가 직렬로 한다. 공통 계약(decisions.md §0)이 확정됐으므로 **WP3(P0-C 확장 create)** 는 WP1과 병렬 가능하고, **WP4(P0-D compose 단절 정리)** 는 WP2 뒤에 한다.

`fixtures/`는 판정에 쓴 임시 fixture(`/tmp/aip-expr-fx`, `/tmp/aip-expr-run`)의 사본이다. `s*.aip`는 `base.inc`를 앞에 붙여 만든 결과물이고, `s05b`는 효과 `update` 산술로 바꾼 실패 변형이다(원형은 `{ balance = 0 }`이며 check 통과). `run_*`은 serve 실행(audit R1·R2)에 쓴 앱이다. 재확인: `aip service check plan-docs/expressiveness-1.0/fixtures/<파일>`.
