# Infisical 포팅 설계 (WBS 7.3.2)

조사 단계의 설계다. 코드는 한 줄도 옮기지 않았고, 실행 검증도 없다.
처리 방식 3분류: **선언 흡수**(app.aip resource/transition/predicate로 직접 표현),
**확장 필요**(공식 Node/Python `extension`·`operation job`으로 감싸 서버가 최종 결정),
**잔여 서비스**(AIP 밖 전문 런타임으로 남기고 capability 경계로만 연결).

7.1.4 교체 표(`plan-docs/porting/README.md`)의 소유 원칙을 따른다:
입력 검증·인가·비용·최종 트랜잭션은 AIP 서버가 소유하고, 원본 ORM(Knex DAL) 프록시는 금지다.

## 모듈별 처리 설계

| ID | 모듈 | 처리 | 설계 |
|---|---|---|---|
| I01 | 사용자 인증 | 선언+확장 | 회원·MFA·WebAuthn 등록 row는 resource, 세션은 AIP auth+principal binding. SRP verifier 검사는 **확장**(서버가 실행, 프런트가 검증값을 결정하지 못함). 비밀번호 평문·verifier를 프런트에 노출 금지 |
| I02 | 머신 아이덴티티 | 확장 중심 | 12종 외부 인증(JWT/OIDC/AWS/GCP/Azure/K8s/TLS/SPIFFE/LDAP 등)은 외부 IdP 검증이 본질이므로 `operation job` 확장으로 감싼다. identity·바인딩 row는 resource. claim은 검증된 것만 소비(FlaskBB provisioning 선례) |
| I03 | 시크릿·폴더 | 선언 중심 | secret/folder/env/version row는 resource+transition, 폴더 트리는 traverse, 공유·승인 흐름은 transition+notify, 검증 규칙은 check. blind-index(E2E 암호문 검색)는 선언 불가 → 갭 G4 |
| I04 | 조직·RBAC | 선언 | org/membership/role row는 resource, CASL 조건식은 predicate+row 정책으로 재표현. 조건식 표현력 차이는 7.3.3에서 대조 필요(잠정 갭 G7) |
| I05 | 암호화·KMS | 경계 결정 필요 | 저장 암호화는 AIP encrypted field로 흡수 가능. 단 E2E(클라이언트 보유 키)·SRP·blind-index는 **서버 보유 키 모델과 충돌**하므로 흡수 불가 → 잔여 또는 모델 변경(창시자 결정). 내부 KMS API 자체의 포팅은 선언+확장으로 가능 |
| I06 | 비동기·알림 | 선언+확장 | BullMQ 큐 1건당 `operation job` 1건 매핑, 주기는 서비스 설정(관례: capability-gap 결정). SMTP/webhook/슬랙 발송은 등록된 전송 capability로만 (FlaskBB 메일 선례) |
| I07 | 연동 | 잔여+선언 | provider 90종 구현은 잔여 서비스(외부 API 전문). 연결 row·동기화 상태는 resource, 동기화 실행은 job. provider별 capability 등록 절차가 필요(갭 G8) |
| I08 | PKI·인증서 | 확장+선언 | 발급(EST/SCEP/ACME)·서명·갱신은 외부 암호 연산이므로 확장 job. 인벤토리·프로파일·정책 row는 선언. X.509 의미 자체는 갭 G6 |
| I09 | 관리·운영 | 선언 | admin predicate+transition, dashboard 집계는 aggregate(집계 meaning 대조는 7.3.3) |
| I10 | 감사·이벤트 | 선언+잔여 | event-outbox→notify/outbox, 조회는 read. ClickHouse 감사 저장은 잔여(읽기 capability로만 연결). EE audit-log는 범위 밖 |
| I11 | 프론트엔드 | 잔여→전환 | React SPA의 data access를 공식 SDK 호출로 전환 (페이스리뷰 UI 규칙과 동일). 화면 전수 대조는 7.3.3 범위 밖 |
| I13 | EE | 범위 밖 | 77 서비스·122 라우터 파일. 참조 목록만 유지 |
| I14 | Go 사이드카 | 잔여 | HSM·license는 EE라 범위 밖, queue 보조는 I06 job으로 흡수 검토 |

## 우회 금지 경계 (AIP 정본·7.1.4 대조)

1. **호출자는 인가를 결정하지 않는다** (`docs/PRINCIPLES.md` §2): 프런트·CLI·SDK가
   role/owner/승인 여부를 판단하는 분기를 가지면 포팅이 아니라 우회다. 서버 predicate가 최종 결정.
2. **호출자는 암호를 결정하지 않는다**: E2E 개인키·SRP verifier·KMS 평문·blind-index
   trapdoor를 프런트·확장 입력으로 주고받지 않는다. 확장은 검증된 claim·ciphertext만 다룬다.
3. **원본 백엔드 브리지 금지** (7.1.4 "원본 ORM proxy 금지"): Knex DAL·Fastify handler를
   그대로 호출하는 어댑터는 포팅이 아니다. DB 접근은 AIP 선언을 통해서만.
4. **외부 검증은 서버가**: IdP 토큰·클라우드 credential 검증은 `operation job` 안에서,
   결과 claim만 선언 세계로 들여온다 (FlaskBB `/session` provisioning 선례).
5. **토큰·세션 수명과 해지는 서버 상태**: 로그아웃 후 구토큰·정지 계정은 7.1.4 gate와
   동일하게 서버가 강제한다.
6. **발송 증명은 전송 receipt**: notify outbox 기록·job enqueue는 메일/웹훅 발송이 아니다
   (7.1.4 이메일 행). sink·실provider 구분은 FlaskBB 메일 선례를 따른다.

## AIP 제품 갭 목록 (7.3.2 확정분)

| ID | 갭 | 근거 |
|---|---|---|
| G1 | 순위 기반 전문검색 선언 불가 | FlaskBB 7.5에서 제품 다이얼렉트 `search` 파서 거부 확인 |
| G2 | E2E/클라이언트 보유 키 모델 | AIP encrypted field는 서버 보유 키(`AIP_ENCRYPTION_KEYS`). zero-knowledge 흡수는 모델 변경 없이 불가 |
| G3 | SRP 패스워드 프로토콜 | AIP auth에 SRP verifier 검증 수단 없음 |
| G4 | blind-index (암호문 검색) | `contains`는 평문 필드용. 암호문 동형 검색 의미 없음 |
| G5 | TTL/lease 동적 시크릿 의미 | job은 있으나 lease 발급·갱신·해지(revoke) 의미 없음 |
| G6 | X.509/EST/SCEP/ACME 발급 의미 | 확장 job으로 감쌀 수 있으나 선언 의미 없음 |
| G7 | CASL 조건식→predicate (잠정) | 폴더 권한 조건식의 표현력 대조가 7.3.3에 남음 |
| G8 | provider capability 등록 절차 (잠정) | 90종 연동의 등록·검증·버전 운영 절차 미정 |

G7·G8은 7.3.3 대조 결과에 따라 갭 확정 또는 해소한다.

## 7.3.3 포팅 착수 작업 분해

작은 단위, 각 항목은 완료 기준·검증 방법을 포함한다. DB·서버 실행이 필요한 항목은
환경이 갖춰진 세션에서 수행한다.

- [ ] 7.3.3.1 revision 잠금·인벤토리 확정 — 본 세션에서 수행. 완료 기준:
  `infisical-inventory.json` 커밋 + `test_infisical_inventory` 2 passed. (완료)
- [ ] 7.3.3.2 SRP 경계 결정 — I01 로그인 흐름의 원본 분기 정독 후 SRP를 확장에 둘지
  잔여로 둘지 결정 문서 1쪽. 완료 기준: 결정문+근거행. 검증: 문서 리뷰(실행 없음).
- [ ] 7.3.3.3 I03 시크릿 resource 초안 — secret/folder/env/version 최소 선언 + `service check`.
  완료 기준: `check ok:true`. 검증: 무DB.
- [ ] 7.3.3.4 I04 RBAC predicate 초안 — org/membership/role + 폴더 읽기 정책 최소 선언 +
  `service check`. 완료 기준·검증: 7.3.3.3과 동일.
- [ ] 7.3.3.5 I06 큐→job 매핑표 — BullMQ 큐(MIT분) 전수에 대한 operation job 선언 초안.
  완료 기준: 매핑표+선언 `check`. 검증: 무DB(선언), 실기동은 DB 세션.
- [ ] 7.3.3.6 I05 암호화 모델 결정 — G2 수용(서버 키) vs 잔여(E2E 유지)를 창시자에게 묻는
  결정 요청문. 완료 기준: 승인된 결정 기록. 검증: 문서.
- [ ] 7.3.3.7 I02 identity 확장 spike — JWT 1종에 대한 검증 확장 목업+단위 테스트(소켓 없음).
  완료 기준: 목업 테스트 통과. 검증: `unittest`.
- [ ] 7.3.3.8 EE 제외 비준 — I13 목록 확정 + 분모 고정 선언. 완료 기준: 본 문서+inventory 커밋.
- [ ] 7.3.3.9 실DB 기준선 (환경 종속) — 격리 PG에 원본 스키마가 아닌 AIP 선언 스키마로
  `init→migrate→seed→serve` 재현. 완료 기준: serve 기동+SDK read 1건. 검증: 실DB.

순서: 7.3.3.8 → 7.3.3.2/7.3.3.6(결정) → 7.3.3.3/7.3.3.4/7.3.3.5(선언) → 7.3.3.7(확장) →
7.3.3.9(실DB).
