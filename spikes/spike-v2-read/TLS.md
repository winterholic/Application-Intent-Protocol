# PostgreSQL 연결 TLS: 8개 검토 관문

이 기록은 `spike-v2-read`의 공통 연결 경로를 바꿀 때 지킬 보안 조건과 검증 관문이다. API 및 migration 호출자는 `connect_owned_with_url`을 사용하고, 연결 생성 전에 주소와 SSL 모드를 검사한다.

## 관문

1. **URL 파싱** — 모든 호출자는 같은 파서와 정책 판정을 거친다. 파싱 실패는 연결 전에 반환한다.
2. **목표 주소 판정** — Unix socket, `localhost`, IPv4/IPv6 loopback만 로컬로 취급한다. `hostaddr`가 있으면 실제 접속 주소도 판정하며, 한 목적지라도 외부면 원격으로 본다.
3. **원격 평문 차단** — 원격 TCP 목적지는 `sslmode=require`만 허용한다. `disable`, `prefer`, 기본 모드는 DNS/소켓 연결 전에 거부한다.
4. **로컬 호환성** — 로컬 `disable`과 `prefer`는 기존 개발 DB와 테스트를 위해 `NoTls`로 연결한다.
5. **Require의 TLS 강제** — 원격과 로컬 모두 `require`일 때 rustls 커넥터를 사용한다. 서버가 TLS를 지원하지 않으면 연결이 실패하며 평문으로 재시도하지 않는다.
6. **서버 인증** — 공개 CA root store와 호스트명 검증을 켠다. `install_ca_bundle`로 추가할 사설 CA는 인증서 PEM만 허용하고 최대 64 KiB이며, 공개 root를 대체하지 않는다. 인증서 검증을 끄거나 `NoTls`로 fallback 하는 옵션은 두지 않는다.
7. **연결 수명** — `OwnedConnection`이 PostgreSQL driver task를 소유하고 Drop에서 중단한다. detached `Client` 경로도 동일한 정책 connector를 이용한다.
8. **행동 검증** — 원격 `disable` 및 `prefer`가 네트워크 동작 전에 거부되고, 로컬 비-TLS 연결이 기존처럼 통과하는지 확인한다. 임시 TLS 서버로 공개/설치 CA와 일치 호스트명의 수락, 잘못된 CA/호스트명의 거부를 검증한다. 설치 함수에는 invalid/empty/과대 PEM 거부, 같은 번들 재호출 수락, 다른 번들 재호출 거부도 확인한다.

## 라이브러리 기준

`tokio-postgres-rustls` 0.14.0은 `tokio-postgres` 0.7 및 rustls 0.23과 호환되는 API를 제공한다. 공개 CA root는 `webpki-roots`를 사용한다. 이 crate는 rustls crypto provider 선택을 애플리케이션에 맡긴다. 구현은 ring provider를 명시적으로 선택한다.

`install_ca_bundle`은 첫 유효 설정을 프로세스 전역에 보관한다. 같은 바이트를 다시 설치하는 것은 허용하고 다른 바이트의 설정은 정책 오류로 거부한다. 따라서 DB 연결을 열기 전에 설정 파일에서 읽은 번들을 한 번 설치해야 한다. `connect_owned_with_url`과 detached `connect_with_url`은 이 번들을 공개 root에 더해 사용한다. 명시적 `connect_owned_with_url_and_ca`는 호출 단위 PEM을 공개 root에 더해 사용한다. 두 경로 모두 인증서 체인과 서버 호스트명을 검증한다.

- [tokio-postgres-rustls 0.14.0 문서](https://docs.rs/crate/tokio-postgres-rustls/latest)
- [MakeRustlsConnect API](https://docs.rs/tokio-postgres-rustls/latest/tokio_postgres_rustls/struct.MakeRustlsConnect.html)
- [tokio-postgres SSL mode](https://docs.rs/tokio-postgres/latest/tokio_postgres/config/enum.SslMode.html)

## 진행 기록

- 구현 전: 위 여덟 관문과 테스트 조건을 기록했다.
- 테스트 우선: 원격 `disable`/`prefer` 테스트가 구현 전에 실패했다. listener가 연결을 받았고 실제 오류는 `error communicating with the server`였다.
- TLS 테스트: loopback `require`의 SSLRequest와 ClientHello, 명시적 CA 및 설치 CA를 신뢰하는 SAN 일치, 잘못된 CA와 잘못된 호스트명 거부를 ephemeral TLS server로 확인했다. 설치 API는 strict certificate-only PEM, 64 KiB 상한, 동일 설정 idempotency, 상이한 설정 거부를 적용한다.
- 최종 검증: `cargo test` 전체가 통과했고, 여기에는 로컬 PostgreSQL 연결을 사용하는 기존 읽기 테스트가 포함된다.
