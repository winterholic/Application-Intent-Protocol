# 인증 경계: SRP·세션 (WBS 7.3.3.2, I01/I02)

원본 근거(실행 없음, 판독만):
`backend/src/services/auth/auth-login-service.ts`가 `srpCheckClientProof`와
`getUserPrivateKey`를 `lib/crypto/srp`에서 가져다 쓴다. 같은 파일에
"Deprecated: provider token validation removed. This path is only reachable
from legacy SRP clients."와 "Deprecated SRP path: default to email auth."라는
주석이 있다. 즉 SRP는 레거시 호환 경로로 남아 있고 기본은 이메일 인증이다.

## 결정

1. SRP client-proof 검증은 서버가 실행한다. 공식 인증 확장에 두고 프런트·SDK·CLI가
   검증 결과를 결정하지 못하게 한다. verifier·salt의 저장 row 접근은 확장의
   선언된 capability로만.
2. `getUserPrivateKey` 계열(사용자 개인키 복호)은 서버 경계 안에 둔다. 개인키·복호값을
   job 입출력·로그·응답에 포함하지 않는다.
3. 세션·토큰 수명과 해지(로그아웃 후 구토큰·정지 계정)는 7.1.4 gate와 동일하게
   서버가 강제한다. 확장이 발급한 토큰의 검증도 서버가 한다.
4. 신규 구현에서는 SRP가 아닌 AIP 기본 auth+principal binding을 우선한다. SRP는
   레거시 클라이언트 호환용으로만 유지하고 신규 경로로 승격하지 않는다.

## I02 확장 spike 상태
`porting/infisical/workers/infisical_jobs.py`의 `verify_identity_jwt`는 JWT 1종
서명+만료+iss/aud 검증을 구현하고 `tests/test_identity_jwt.py` 7 passed.
나머지 11종(AWS/GCP/Azure/K8s/OIDC/LDAP/TLS/SPIFFE/token 등)은 미구현.
외부 IdP 검증은 항상 서버(확장) 안에서 끝나고, 선언 세계에는 검증된
`(issuer, subject, claims)`만 들어간다.
