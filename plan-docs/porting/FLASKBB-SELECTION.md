# 공개 원본 후보 비교와 고정 결정

## 결정

AIP 전체 앱 포팅 대상은 **FlaskBB 2019-10-25 고정본**이다.

- 저장소: <https://github.com/flaskbb/flaskbb>
- 고정 원본 SHA: `0885238df7c54826adda8cfbe8a831e846e2cb64`
- 고정 시점: 2019-10-25 22:33:05 +02:00
- 태그를 최신판으로 따라가지 않는다. 선택 당시 공개된 전체 Git 저장소의 이 SHA를 기준으로 한다.
- 별도 읽기 전용 공개 저장소 clone: `/private/tmp/flaskbb-source-aip`
- 동일 SHA의 보존용 source snapshot: [`porting/flaskbb/upstream/`](../../porting/flaskbb/upstream/)
- snapshot의 원본 `README.md`, `LICENSE`, `AUTHORS`, `NOTICE`, `CONTRIBUTING.md`, `CHANGES`를 포함해 하위 라이선스 고지를 보존한다.

## 후보 비교

| 후보 | 같은 시기의 제품 범위 | 포팅 적합성 | 판단 |
|---|---|---|---|
| FlaskBB | 실제 운영형 게시판: 사용자·그룹/포럼별 권한·카테고리·포럼·토픽·게시글·미읽음/추적·검색·첨부·관리 UI·플러그인. 당시 README는 private messages, admin, group permissions, markdown, attachments와 plugin 기반 기능을 설명한다. | Flask + SQLAlchemy 관계 모델이며 upstream 기본 설정에 PostgreSQL URI가 예시돼 있다. 템플릿 UI와 서버가 가진 업무 규칙이 함께 있어 UI부터 DB까지 전체 이식 검증에 적합하다. 구현 규모는 커뮤니티 전체 플랫폼보다 감당 가능하다. | **선정.** 기능 축소 없이 공개 저장소의 고정 코드 전체를 실행해 분모로 삼는다. |
| django-wiki | 계층형 위키, 문서 편집/버전, 권한, 첨부, 검색을 제공하는 Django 앱. | PostgreSQL 관계형 모델과 문서 권한/버전 업무를 대조하기 좋으나 포럼의 사용자 상호작용·게시글·개인 메시지/관리 흐름보다 범위가 좁다. | 후보. 충분한 완성 앱이지만 FlaskBB의 다중 역할/관계 업무를 포팅 검증하는 쪽이 이번 출시 gate에 더 유익하다. |
| wallabag | 자가 호스팅 read-it-later 앱: URL 저장, 본문 추출, 분류/읽기, API와 주변 앱 생태계. | 웹 UI·데이터 관리가 실재하나 본문 추출과 외부 서비스/클라이언트 생태계가 검증 범위에 크게 들어간다. 완성된 서버의 범위는 FlaskBB보다 크다. | 후보. 이번 작업의 고정 대상으로 선택하지 않았다. |

후보의 제품 설명은 각 공식 저장소 및 공식 release/문서 기준으로 비교했다. 후보 비교는 README 주제 1개 구현을 완료로 세지 않는다.

## 작성 시기와 저작 이력의 한계

원본 Git 이력은 2013년 9월부터 이어지고 2019년 고정본 직전까지 여러 이름의 기여자와 병합/수정 이력이 보인다. 고정 커밋 작성자는 Peter Justin이며 해당 커밋은 "Get involved users of a topic before deleting it" 변경이다. 고정본 `LICENSE`의 저작권 표기는 FlaskBB Team 및 contributors, 2013–2019이고 `AUTHORS`와 `NOTICE`가 같은 원본 안에 있다. 이 이력과 코드가 AI 코딩 도구가 보편화되기 이전에 축적된 코드라는 정황을 뒷받침하지만, 모든 줄의 인간 저작을 증명하지는 않는다. 이 프로젝트의 출발 연도만으로 2019 시점 코드 전부가 인간 작성이라고 주장하지 않는다.

기준 커밋은 GitHub 공식 원본에서 고정 SHA로 checkout했고 커밋 날짜, 상위 이력, 당시 README/라이선스/기여자 목록을 로컬 Git object에서 확인했다. 원본 clone은 수정하지 않는다. snapshot은 AIP 포팅 작업이 같은 원본을 재현할 수 있도록 만든 보존본이며 자체 라이선스를 추가하지 않는다.

## AIP 범위 경계

고정본과 그 고정본이 기본 의존성으로 설치/활성화하는 공식 FlaskBB plugin은 기준 기능으로 포함한다. 미포함 플러그인, 프레임워크 생성 문서 경로, 운영자가 따로 설치하는 선택 서비스는 upstream의 당시 설치 문서와 코드에 근거해 각각 표시한다. 기능이 복잡하거나 AIP의 현재 계약으로 표현하기 어렵다는 이유만으로 제외하지 않는다. 선택 이후 구현 난이도만으로 대상을 바꾸지 않는다.

FaceReview의 기존 WBS, MySQL adapter 및 browser blocker는 별도 미완료 포팅 track으로 그대로 둔다. FlaskBB를 선택했다는 이유로 그 항목을 대체/해결/종료 처리하지 않는다.
