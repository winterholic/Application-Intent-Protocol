# Infisical 포팅 설계 (WBS 7.3.2) — 2026-10-18 정정판

원본은 실행하지 않았고 소스 판독과 파일 집계로 조사했다. 이 문서는 처음 설계와 체크리스트를 독립 검증 결과에 맞춰 고쳤다. 두 가지 구분을 일관되게 쓴다.

- **동작 확인**: 격리 PostgreSQL에서 AIP 서비스를 실제로 띄우고 요청으로 확인했다(근거 명령과 출력 링크를 단다).
- **선언만 / 설계만 / 문서만**: 정의를 쓰고 `aip service check`만 통과했거나, 문서에만 있다. 동작을 주장하지 않는다.

처리 방식 3분류: **선언 흡수**(app.aip resource/transition/predicate로 직접 표현), **확장 필요**(공식 Node/Python `extension`·`operation job`으로 감싸 서버가 최종 결정), **잔여 서비스**(AIP 밖 전문 런타임으로 남기고 capability 경계로만 연결).

7.1.4 교체 표(`plan-docs/porting/README.md`)의 소유 원칙을 따른다. 입력 검증·인가·비용·최종 트랜잭션은 AIP 서버가 소유하고, 원본 ORM(Knex DAL) 프록시는 금지다.

## 정정 요약

| 이전 서술 | 정정 | 문서 |
|---|---|---|
| 원본은 E2E, AIP는 서버 키라 충돌, 창시자 결정 필요 | 원본 기본 경로가 서버측 KMS라 충돌 없음. 결정 요청 철회 | [KEYMODEL](INFISICAL-KEYMODEL.md) |
| SRP 검증을 서버 확장으로 | SRP는 원본에서도 레거시. 로그인은 AIP 밖 IdP 몫 | [AUTHN](INFISICAL-AUTHN.md) |
| 제품 갭 G2는 E2E 키 모델 | G2는 `encrypted` 수정자·키 공급이 제품에 없다는 갭으로 교체. 삭제·create 효과 등 갭 추가 | [GAPS](INFISICAL-GAPS.md) |
| 라우트 903, 라우터 425개 중 EE 제외, 큐 16개 | 라우트 정의 1002, MIT 라우터 303, 큐 54개 | [SELECTION](INFISICAL-SELECTION.md) |
| EE 제외 비준 | 비준 철회. 권한 핵심이 EE에 있고 MIT 파일 446개가 EE를 import. 사용자 판단 대기 | [EE-BOUNDARY](INFISICAL-EE-BOUNDARY.md) |
| 큐 16개를 job 16개로 선언 | 구현 없는 선언은 실패하므로 제거. 구현은 1개 | [JOBS](INFISICAL-JOBS.md) |
| app.aip 초안(권한·키·삭제·목록) | 실DB 결함 10건을 재현하고 수정. 수정 후 17건 통과 | 아래 7.3.3.3/4 |

## 모듈별 처리 설계

"현재 상태"는 위 구분을 따른다. 표시가 없는 모듈은 미착수다.

| ID | 모듈 | 처리 | 설계 | 현재 상태 |
|---|---|---|---|---|
| I01 | 사용자 인증 | 선언+IdP | 로그인·비밀번호·MFA·refresh는 AIP 밖 IdP. AIP는 principal binding과 `User` resource. 비밀번호·verifier는 AIP에서 다루지 않음 | `User` 행 정책만 **동작 확인**. 나머지 미착수 |
| I02 | 머신 아이덴티티 | 확장 중심 | 13개 인증 방식(universal-auth + `identity-*-auth` 12개)은 외부 검증이 본질이므로 `operation job`으로 감싼다. claim은 검증된 것만 소비 | JWT 1종 순수 함수 단위시험 7건(서비스 미연결, **서비스에서 도달 불가**). 나머지 12개 미구현 |
| I03 | 시크릿·폴더 | 선언 중심 | 폴더(환경마다 루트 `/`), 시크릿(폴더+이름 유일)과 물리 삭제를 resource+transition으로. 값·버전·blind index는 갭 G2/G4 | 이름·폴더·물리 삭제 **동작 확인**(원본 대조: [GAPS G10](INFISICAL-GAPS.md)). **값은 없음**. 공유·승인·태그·import·버전 미착수 |
| I04 | 조직·RBAC | 선언 | 조직·프로젝트·멤버십·역할을 predicate와 행 정책으로. 원본 CASL 조건식과의 대조는 EE 경계 문서 | 역할 기반 규칙(admin/member/viewer/no-access) **동작 확인**. 조건식·커스텀 역할·그룹 미착수 |
| I05 | 암호화·KMS | 갭 대기 | 원본이 서버측 KMS이므로 AIP의 `encrypted` 필드(설계 13, Proposed)가 대응 수단. 제품에 없어 선언할 수 없다 | **미착수**(제품 갭 G2). core 수정 금지 |
| I06 | 비동기·알림 | 선언+확장 | 큐 54개 중 MIT 30(+미참조 3)을 job으로. 발송은 전송 capability 경유, 알림 기록은 전송이 아님 | 큐 1개만 부분 구현(알림 기록, **동작 확인**). 53개 미선언 |
| I07 | 연동 | 잔여+선언 | provider 구현은 잔여 서비스. 연결 row·동기화 상태는 resource, 실행은 job | 미착수 |
| I08 | PKI·인증서 | 확장+선언 | 발급·서명·갱신은 확장 job. 인벤토리·프로파일·정책은 선언 | 미착수 |
| I09 | 관리·운영 | 선언 | admin predicate+transition, dashboard는 aggregate | 미착수 |
| I10 | 감사·이벤트 | 선언+잔여 | event-outbox→notify/outbox. ClickHouse 감사 저장은 잔여. EE audit-log는 범위 판단 대기 | AIP 내장 감사 행(`aip_audit`)이 job·apply 전이마다 기록되는 것을 **관찰**(`evidence/baseline-10-audit.txt`). 원본 감사 모델과의 대조는 미착수 |
| I11 | 프론트엔드 | 잔여→전환 | React SPA의 data access를 공식 SDK 호출로 전환. 화면 전수 대조는 범위 밖 | 미착수 |
| I13 | EE | 판단 대기 | 77 서비스, 122 라우터 파일. 범위는 사용자 판단 | — |
| I14 | Go 사이드카 | 잔여 | HSM·license는 EE | 미착수 |

## 우회 금지 경계 (AIP 정본·7.1.4 대조)

1. **호출자는 인가를 결정하지 않는다** (`docs/PRINCIPLES.md` §2). 프런트·CLI·SDK가 role/owner/승인 여부를 판단하는 분기를 가지면 포팅이 아니라 우회다. 서버 predicate가 최종 결정한다. 역할 검사가 필요한 쓰기는 `allow`가 서버에서 평가한다.
2. **호출자는 암호를 결정하지 않는다.** 키·평문·trapdoor를 프런트나 확장 입력으로 주고받지 않는다. 평문 열을 암호문이라 부르며 흉내 내지 않는다(G2 처리 방식).
3. **원본 백엔드 브리지 금지.** Knex DAL·Fastify handler를 그대로 호출하는 어댑터는 포팅이 아니다. DB 접근은 AIP 선언을 통해서만.
4. **외부 검증은 서버가.** IdP 토큰·클라우드 credential 검증은 `operation job` 안에서, 결과 claim만 선언 세계로 들여온다.
5. **토큰·세션 수명과 해지는 서버 상태.** 로그아웃 후 구토큰·정지 계정은 `principal revoke`와 토큰 수명 900초 이하 프로필로 서버가 강제한다.
6. **발송 증명은 전송 receipt.** notify outbox 기록·job enqueue는 메일/웹훅 발송이 아니다.
7. **선언은 동작하는 것만 서비스한다.** 구현 없는 `operation job`은 enqueue가 받아지고 나중에 `EXTENSION_ERROR`로 실패한다. 그런 선언은 두지 않는다.

## 제품 갭

[INFISICAL-GAPS.md](INFISICAL-GAPS.md)로 이동했다. 요약: G2 `encrypted` 수정자·서버 키 공급 부재(중요), G10 행 삭제·부분 유일 제약 부재, G11 create의 관련 행 동시 생성 부재, G12 create 결과 id 전달 조합 부재. G3(SRP)는 갭이 아님으로 분류를 바꿨고 G5는 EE로 옮겼다.

## 7.3.3 포팅 착수 작업 분해와 현재 상태

범례: ✅ 동작 확인, 🟡 부분(범위 명시), 📄 문서만, ⏸ 판단·선행 대기, ⬜ 미착수.

- 🟡 **7.3.3.1 revision 잠금·인벤토리** — 라우트 정의 1002, 라우터 MIT 303 + EE 122, 서비스 디렉터리 126개 전부 배정, 큐 54개 분류, EE import 결합도. `tools/test_infisical_inventory.py` 14건과 재생성 동일성. 라우트 수는 정의 수이고 노출 endpoint 수가 아니며, 모듈 배정은 이름 기반 휴리스틱이다.
- 📄 **7.3.3.2 SRP·인증 경계** — [AUTHN](INFISICAL-AUTHN.md). 소스 판독이며 동작 확인 없음. SRP는 승격하지 않고 로그인은 IdP 몫으로 정정.
- ✅🟡 **7.3.3.3 I03 시크릿 resource** — `porting/infisical/app.aip`. 폴더 안 시크릿 이름 유일(원본 부분 유일 인덱스와 같은 집합)·물리 삭제·삭제 뒤 이름 재사용·일괄 삭제·삭제 감사·viewer 쓰기와 삭제 거부를 실DB로 확인(`test_live_db.py` 20건). 2026-10-20 이전의 `SecretKey`/세대/소프트 삭제 근사는 원본과 달라 제거했다. **시크릿 값은 선언하지 않았다**(G2). 값·버전·회전·blind index는 ⬜.
- ✅🟡 **7.3.3.4 I04 RBAC predicate** — 같은 파일. admin/member/viewer/no-access, 조직 admin 암묵 접근, 조직 격리, 프로젝트 멤버 추가 규칙, 조직 생성자 첫 admin, 사용자 가시성을 실DB로 확인. 원본 CASL 조건식, 커스텀 역할, 그룹, 비율 제한은 ⬜. 규칙마다 약화하면 시험이 실패하는지 `tests/mutation_check.py`로 확인(8개 잡힘, 2개는 다른 규칙과 중복이라 동등 변이로 표시).
- 🟡 **7.3.3.5 큐→job 매핑** — [JOBS](INFISICAL-JOBS.md). 54개 큐 분류표. job 구현은 `secretReminder` 1개(알림 기록, 전송 아님)이고 53개는 미선언. 이전의 "16개 job 선언 완료"는 철회.
- 📄 **7.3.3.6 I05 키 모델** — [KEYMODEL](INFISICAL-KEYMODEL.md). 원본이 서버측 KMS임을 판독으로 확정, 결정 요청 철회. 구현은 ⏸(제품 갭 G2, 설계 13은 승인 전이고 독립 정적 리뷰가 현 초안을 구현에 넘기지 말라고 판정했다. 상세는 GAPS의 G2).
- 🟡 **7.3.3.7 I02 identity 확장 spike** — `workers/infisical_jobs.py`의 JWT 검증 순수 함수와 `tests/test_identity_jwt.py` 7건 통과(DB 없음). `app.aip`에 연결된 operation이 없어 서비스에서 도달할 수 없다. 나머지 12개 인증 방식 ⬜.
- ⏸ **7.3.3.8 EE 제외 비준** — 철회. [EE-BOUNDARY](INFISICAL-EE-BOUNDARY.md)의 사용자 판단 항목 5개 대기.
- ✅🟡 **7.3.3.9 실DB 기준선** — `porting/infisical/DB-RUNBOOK.md`와 `run-baseline.sh`를 새 컨테이너·새 venv에서 처음부터 실행해 종료 코드 0, SDK 점검 11개 통과(2026-10-20 재실행은 10개, 이름·세대 분리 제거로 점검 하나가 합쳐짐), job `SUCCEEDED`, 감사 행 확인(`porting/infisical/evidence/baseline-*.txt`). 기준선이 다루는 범위는 위 7.3.3.3/4와 job 1종이다. 원본 Infisical과의 동작 동등성은 비교하지 않았다.

순서: 7.3.3.8(판단) → 7.3.3.2/7.3.3.6 → 7.3.3.3/7.3.3.4/7.3.3.5 → 7.3.3.7 → 7.3.3.9. 값이 필요한 후속(I03 값·버전, I05, 회전, 동기화)은 G2 해소가 선행이다.
