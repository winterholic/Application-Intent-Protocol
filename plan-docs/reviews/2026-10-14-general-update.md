# 일반·부분 수정 구현 기록

2026-10-14. 작업 위치 `/tmp/aip-wt-update`, 시작 `origin/main` 9de48e4. 다른 작업자의 UUID Id·DB 채택·검증 복구와 분리한다.

## 설계 관문

1. 원칙 1·2·3·4·7: 메모 편집 같은 CRUD의 값 검사·인가·원자성을 공통 엔진으로 옮긴다.
2. 기존 공개 전이에 `patch title, content`를 선언해 호출자가 해당 필드 값만 지정할 수 있다. 타입·길이·nullable는 resource fields의 선언을 재사용한다.
3. 앱마다 update DTO·endpoint·SQL을 만들 필요가 줄어든다.
4. rows read·allow는 변경 전 잠근 행으로 평가한다. 모르는 값 키·기본키 변경은 거부한다. 참조 변경은 생성과 같은 가시성 검사를 적용한다.
5. 다섯 작성 형식을 동일 AST/facts로 하강한다. TS/Python 호출은 기존 apply에 values·expect를 더한다.
6. 별도 update endpoint나 입력 타입 복제 없이 `patch` 하나로 수정 허용 필드를 선언한다. `expect version`은 호출자가 읽은 version과 현재 행의 같음을 요구하며 `to version = version + 1`과 조합해 낙관적 잠금을 만든다. 일반 조건부 갱신도 `expect title, content`로 같은 방식이다. expect가 없는 선언은 행 잠금 아래 마지막 쓰기가 이긴다.
7. 기존 전이·effect·notify·check·audit·멱등·재시도 transaction 경로를 확장한다. 기존 전이 facts와 생성 계약은 patch/expect가 없으면 보존한다.
8. 사용자 2026-10-14 일반 수정 구현 지시의 범위다. 최종 작성 언어·Id wire 선택은 닫지 않는다.

## 실행 의미

`transition edit { from deleted = false; allow owner = actor; patch title, content; expect version; to version = version + 1; notify owner "note.edited" }`

`apply({apply:"Note.edit",target:{ids:[id]},values:{title:"새 제목"},expect:{version:3}}, {key})`.

values는 비어 있지 않은 객체이며 patch allowlist의 부분집합이다. 누락은 유지, 명시적 null은 nullable만 허용한다. expect 선언은 모든 키를 요구한다. 조건은 인가 뒤 잠근 행에 대해 NULL-safe 비교한다. 불일치면 WIRE.CONFLICT, 전체 rollback. 같은 key 재전송은 기존 확정 결과를 재생한다. conflict는 최신 read 뒤 새 values/expect와 새 key로 요청한다. patch와 고정 to에 같은 필드를 넣거나 patch와 repeat unchanged를 조합하면 정의 오류다. 가려진 필드를 expect로 비교하는 추론 통로는 허용하지 않는다.

## 진행

- 지정 문서·포팅 보고서·최근 work-history 확인. PostgreSQL 17.11 연결 확인.
- 다음: 선언 parity/음성·실 PostgreSQL·SDK 타입 실패 테스트 → 구현 → 포팅 복사본·회귀 → rebase/push.
