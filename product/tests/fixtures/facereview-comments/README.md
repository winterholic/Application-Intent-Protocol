# 페이스리뷰 댓글 적용 시험

2026-10-15 사용자 지시: AIP 미완료 작업을 이어가며 페이스리뷰 같은 다른 개인 프로젝트 또는 유명 GitHub 공개 프로젝트(Infisical 등)로 실제 적용·시험해도 된다. 이번 묶음은 개인 프로젝트의 댓글 흐름을 먼저 선택했다.

원본 페이스리뷰의 댓글 작성·조회·수정·삭제를 AIP 제품 경로에 옮겼다. 원본 저장소와 운영 데이터를 바꾸지 않고, UUID·관계·DB 기본값을 가진 PostgreSQL 복사 구조에 `introspect → adopt-external → provision → principal bind → serve`를 실행한다. 매 실행마다 새 DB와 제한 runtime 역할을 만들고 종료 시 자신이 만든 DB·역할을 정리한다.

## 원본과 시험 범위

원본: 개인 저장소 `projects/facereview/new-facereview`, 확인 시 HEAD `7948b7e721837fdb73c427cb54e12cad4ec2d5af`.

| 근거 | 옮긴 동작 |
|---|---|
| `app/models/{user,video,comment}.py` | UUID 사용자·영상·댓글, 작성자/영상 FK, 수정·삭제 플래그, 생성·수정 시간 |
| `app/services/watch_service.py`의 댓글 메서드 | 공개 댓글 조회, 작성자만 수정·삭제, 활성 영상에 댓글 작성, 댓글 soft delete |
| `app/schemas/watch.py` | 댓글 1..500자 |
| `app/routes/watch.py` | 게스트 조회 허용, 인증된 사용자 쓰기 |

원본 RDB는 MySQL/MariaDB다. 이 시험은 PostgreSQL에 관계형 구조를 재현한 것으로, **기존 MySQL에 직접 연결하거나 원본 프런트와 API 호환을 검증한 결과가 아니다**. 원본의 UUID 문자열은 PG UUID로, ORM 시간 기본값은 PG `timestamptz` 기본값으로 바꿨다. `version`은 AIP 편집 충돌 검증을 위해 추가했다. 원본은 마지막 편집이 이기는 방식이며 CAS가 구현됐다고 주장하지 않는다. 사용자·영상의 전체 필드, 관리자 조회, 좋아요·북마크 토글, Mongo 감정 타임라인, Redis 세션, Socket.IO, Celery·YouTube 연동은 이번 시험에 포함하지 않는다.

## 실행

PostgreSQL 15 이상, 로컬 `host=localhost dbname=postgres`에 시험 DB·역할을 만들 수 있는 계정, Node·Python·Rust와 SDK TypeScript 도구가 필요하다. 인증 키는 메모리에서 새로 생성하며 저장된 인증 정보나 `.env`를 읽지 않는다.

```sh
cargo build -p aip-cli --locked
node --test product/tests/facereview-comments.test.mjs
cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --target-dir target
```

`tools/verify.sh`에도 이 시험을 넣었다. TS SDK는 패키지를 빌드·pack·설치한 뒤 실행한다. Python은 생성 helper와 저장소의 Python SDK 패키지 경로를 사용한다. Python 배포 패키지 설치 시험으로 세지 않는다.

## 확인하는 경계

- UUID/FK/default introspection과 업무 DDL 없는 채택, 같은 계획 재적용.
- runtime 역할의 DDL 비소유, 원본 인가 행 잠금에 필요한 컬럼별 UPDATE 권한. SELECT-only 실패 시 SQLSTATE 42501과 0개 생성 행을 확인한다.
- 생성·조회와 작성자 관계 조회, 게스트 읽기/쓰기 차단, 공개 댓글의 타인 수정·삭제 차단.
- 작성자 위장, 삭제된 영상 참조, 임의 기본값·version 입력 차단.
- 빈 문자열·501자 emoji·null·숫자 거부, 500자 emoji 수락. SDK 검증을 우회해 서버 경계를 직접 호출한다.
- 수정 플래그·시간·version·audit, 같은 version으로 동시 수정 시 한 요청만 성공.
- 뒤 동작의 권한 거부 시 앞 수정도 rollback, 커밋 응답 유실 후 같은 key 복구와 version 1회 증가.
- TS 생성 타입과 음성 타입 검사, Python 생성 helper, soft delete 뒤 행 보존·조회/수정 차단.
- 영상이 뒤에 삭제된 경우 영상별·일반 댓글 조회에서 모두 숨김. `rows read`에 댓글·영상의 활성 조건을 함께 둔다.
- 원본 기본값 drift 뒤 쓰기 거부와 readiness 503.

다른 SDK 인스턴스에서 쓴 결과를 확인할 때는 새 client로 읽는다. SDK 로컬 캐시의 인스턴스 간 동기화를 이 시험의 보장으로 세지 않는다.

## 시험으로 드러난 수정

외부 PG 채택 상태에서 생성의 `literal()`이 필드 range를 확인하지 않아 `Text(1..500)`에 빈 값을 저장했다. 같은 함수는 Time/Url 생성도 처리하지 못했고 Text의 NUL 검사를 공통 scalar parser와 공유하지 않았다. 새 unit test는 수정 전 2개 실패했고, 수정 뒤 생성·patch·expect 입력이 같은 scalar/range 검증을 사용한다. bundle create와 compose의 constant 입력도 같은 `literal()`을 거친다. 서버가 읽은 compose item 값, 기존 DB default의 의미 검증 전체까지 이번 시험의 검증 범위로 넓히지 않는다.

앱 선언의 첫 버전은 `Comment.is_deleted`만 제한해 일반 조회에 삭제된 영상의 댓글이 남았다. 일반 조회까지 실패를 재현하고 `video.is_deleted = false`를 추가했다. 이는 앱 정책 보완이며 모든 Ref 대상의 가시성이 자동으로 부모 행을 숨긴다는 규칙을 추가한 것이 아니다.

## 다음 실전 적용의 우선순위

1. 페이스리뷰 원본 DB에 붙이려면 MySQL/MariaDB adapter의 introspection·UUID 문자열·transaction·잠금·default·drift 계약이 필요하다. 현재 PG 기본 권장은 유지한다.
2. 좋아요·북마크 원본 토글은 물리 DELETE를 쓴다. AIP의 soft-state로 바꾸려면 취소·재활성화·동시 토글·unique 의미를 별도로 결정하고 시험한다. 같은 동작이라고 간주하지 않는다.
3. 감정 타임라인·Socket.IO·Redis·Celery는 공통 infrastructure capability와 Python 확장의 별도 적용 대상으로 남는다.
4. 다른 공개 프로젝트 후보로 Infisical을 사용할 수 있다. 실제 공개 source revision·라이선스를 먼저 고정하고 조직/멤버십/환경·경로 권한의 작은 slice부터 시험한다. 비밀 암호화·key rotation·전체 서비스 이식은 이번 결과에 포함하지 않는다.

## 설계 관문

1. 서버 반복 구현을 줄이고 호출자의 read/apply 표현을 유지하는 원칙 1·2·3·7을 확인한다. 새 DSL은 없다.
2. 기존 create/read/apply/bundle 범위를 유지한다. 이번 범위 검증 수정은 이미 선언한 제한을 실행에 일치시킨다.
3. 앱별 HTTP controller를 추가하지 않는다. 앱 작성물은 정책·계약과 물리 매핑이며 외부 DB·키·권한은 시험 환경에서 설정한다.
4. 서버가 UUID·입력 타입/범위·행 정책·작성자·CAS·멱등 key를 검증한다. caller의 user/version/default를 권한 근거로 신뢰하지 않는다.
5. 설치 TS SDK와 Python 생성 helper를 같은 서버 계약으로 사용한다.
6. 생성·수정 입력의 scalar 검증을 통합해 별도 표현/검증 모델을 만들지 않는다.
7. 기존 제품 CLI·엔진을 사용하며 IR·컴파일러 구조는 변경하지 않는다.
8. MySQL 지원·물리 delete·최종 작성 언어 같은 Open 결정은 확정하지 않는다. 독립 시험만 추가한다.
