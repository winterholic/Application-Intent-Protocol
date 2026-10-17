# FaceReview MySQL 읽기 포팅 fixture (WBS 7.2.3)

원본 FaceReview 서버(new-facereview@7948b7e721837fdb73c427cb54e12cad4ec2d5af)의 RDB 모델 11개 중 9개를 실제 MySQL 스키마 위에서 AIP로 읽는다.
실험 baseline이며 운영 서버와 같다는 주장은 하지 않는다.

| 파일 | 출처 |
|---|---|
| `schema.sql` | 고정 revision의 SQLAlchemy 모델을 `db.metadata.create_all`로 MySQL 8.4에 만든 뒤 `SHOW CREATE TABLE`로 뽑은 11개 표. 사람이 고치지 않았다 |
| `db-defaults.sql` | 이 포팅이 추가한 DBA 단계. 원본은 `created_at`·`updated_at`·`is_deleted` 같은 값을 Python(`default=`, `onupdate=`)에서 채워 표에 DB default가 없다 |
| `seed.sql` | 이 시험용 행. 숨김 영상, 플래그 NULL 영상, 삭제된 댓글, 음수 포인트, 비로그인 시청 기록을 일부러 넣었다 |
| `facereview.aip` | 9개 resource의 읽기 정의, 사용자 쓰기 조각(댓글 생성·수정·soft 삭제, 좋아요·북마크·영상 요청 생성), 관리자 권한 조각(사용자 비활성화·권한 변경, 요청 거절, 댓글·영상 soft 삭제) |
| `mapping.json` | 필드 이름과 실제 열 이름(`id`→`video_id` 등) 대응. 스키마 이름은 시험이 덮어쓴다 |
| `reference/admin.py` | 같은 방식의 권한 비교(원본 `admin_required`·`super_admin_required`와 AdminService 효과). 5명(GENERAL 2, ADMIN 2, SUPER_ADMIN 1)으로 확인 |
| `reference/writes.py` | 같은 방식의 쓰기 비교(원본이 남기는 행 기준). compare.py와 같은 한계 |
| `reference/compare.py` | 원본 앱의 SQLAlchemy 쿼리와 AIP 응답을 같은 질문으로 비교한 1회성 하네스. 경로·접속 정보는 그 실행 환경의 것이다 |

## 실행

```sh
scripts/mysql-live-fixture.sh up        # 일회용 MySQL 8.4 컨테이너
eval "$(scripts/mysql-live-fixture.sh env)"
cargo test -p aip-cli --features aip-cli/mysql --test mysql_facereview_reads -- --ignored
```

## 확인한 것

- DB default 없이 `adopt-external`을 계획하면 `default database 선언에 실제 DB default가 없음`으로 거절되고, `db-defaults.sql` 뒤에는 업무 DDL 0건으로 통과한다.
- 원본 쿼리와 같은 결과(`reference/compare.py`, 21개 비교 모두 일치): 제목·채널 검색, 카테고리, 최신순 목록과 NULL·삭제 플래그 제외, offset 페이징 1~4쪽, 영상별 댓글(삭제된 댓글·삭제된 영상의 댓글 제외), 본인 전용 표(좋아요·북마크·요청·시청 기록·선호 장르·포인트), 사용자 본인 행과 필드 값(enum 대소문자, 널 허용 정수·불리언), 비밀번호 열 비공개.
- 쓰기(시험과 `reference/writes.py`): 댓글 생성이 UUID v4 id, DB default 플래그, UTC `created_at`을 남기고, 수정은 `is_modified=1`·`updated_at`을 바꾸고, 삭제는 행을 지우지 않고 `is_deleted=1`(원본의 soft delete)로 둔다. 남의 `user`로 만들기, 삭제된 영상에 댓글, 익명 생성, 남의 댓글 수정, 이미 삭제된 댓글 재삭제는 거절되고 행이 남지 않는다. 같은 키 재전송은 다시 쓰지 않는다. `(user, video)` 유일 인덱스는 `WIRE.ALREADY_EXISTS`로 확정 거절된다. 영상 요청은 DB default `PENDING`을 쓴다.
- 권한(시험과 `reference/admin.py`, 판정은 호출자 자신의 행): 회원은 자기 행만, ADMIN·SUPER_ADMIN은 전 사용자를 읽는다. 비활성화는 SUPER_ADMIN은 누구나, ADMIN은 GENERAL만이고 같은 트랜잭션에서 그 회원의 댓글을 숨긴다. 권한 변경은 SUPER_ADMIN만, 활성 회원에게만, enum 안의 값만. 영상 요청 거절은 ADMIN 이상이 PENDING에 한 번만. 댓글·영상 soft 삭제는 ADMIN 이상이고 행은 남는다. 거절된 호출은 행을 바꾸지 않는다.
- 비활성화된 회원은 바로 무효 호출자다. 원본 `login_required`가 `is_deleted=False`만 통과시키는 규칙을 모든 정책에 `active(actor)`로 적었다. 이 규칙이 없으면 비활성화한 회원이 자기 행을 읽고 댓글도 쓸 수 있었다(admin.py가 처음에 3건 실패로 드러냄, 시험도 그 줄을 빼면 실패함을 확인).
- 이 과정에서 어댑터 결함 셋을 고쳤다. (1) ENUM 값을 소문자로 바꿔 비교해 `'GENERAL'`을 가진 실제 스키마가 거절됐다. (2) 표현식 기본값 `DEFAULT (UTC_TIMESTAMP())`의 `EXTRA=DEFAULT_GENERATED`를 generated 컬럼으로 오인해 그 표의 쓰기를 거절했다. (3) 런타임 계정에 `CREATE TEMPORARY TABLES`가 없을 때 서버 능력 probe가 서버 결함처럼 보고하던 것.

## 아직 되지 않는 것(조용히 흉내 내지 않고 남긴 차이)

- 제목 OR 채널 검색. 읽기 `filter`는 AND만 있다.
- 목록 응답의 전체 건수(`total`). 원본은 매 페이지에 돌려주지만 AIP 읽기 응답에는 없다.
- `user_emotion_dna`(JSON 열)와 `user_emotion_summary`(PK가 FK, 낙관적 잠금 열)는 정의하지 않았다.
- 음수 범위 `Int(-1000..1000)`은 문법이 지원하지 않아 `user_point_history.amount`는 범위 없는 `Int`다.
- Mongo(감정 분포)·Redis에 의존하는 목록(`get_all_videos`, 북마크 영상, 추천)은 이 fixture 범위 밖이다.
- 비활성화된 회원의 요청은 원본의 401(`AUTH_INVALID_TOKEN`)이 아니라 200과 빈 결과(쓰기는 거절)다. 인증은 외부 토큰 검증이고 가시성은 정책이라서다. 상태 코드까지 같게 하려면 actor 유효 조건을 정의 수준에서 선언하는 기능이 필요하다.
- 관리자용 별도 resource. 한 표에 resource 둘을 연결하는 매핑은 어댑션이 거절한다(표당 resource 하나). 그래서 관리자는 같은 resource의 정책(`isAdmin`)으로 풀었고, 그 결과 ADMIN은 사용자 화면의 댓글 목록에서도 삭제된 댓글을 받는다(`is_deleted.eq` 필터로 거를 수 있으나 호출자가 정한다).
- 관리자 요청 승인(영상 생성)은 승인자가 고르는 제목·채널·길이·카테고리가 요청 행에 없어서, 그리고 Mongo 초기 분포 쓰기가 따라와서 정의하지 않았다.
- 좋아요·북마크 취소(실제 행 삭제). 정의 언어에 물리 삭제가 없다. 같은 요청을 두 번 보내는 토글 의미를 흉내 내지 않고 `WIRE.ALREADY_EXISTS`로 남긴다.
- 비밀번호 로그인·refresh·logout 등 인증 자체의 이전. MySQL adapter는 서버 확장(워커)을 아직 지원하지 않아 Werkzeug 비밀번호 검사를 AIP 안에서 할 수 없다(7.2.2의 남은 부분).
- 사용자 생성(가입)은 비밀번호 해시를 서버 확장이 만들어야 해서 정의하지 않았다.
