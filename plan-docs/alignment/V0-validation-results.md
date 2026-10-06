# V0. 문서 구조 검증 결과

## 2026-10-04 V15 이후 검사

```text
python3 tools/check_plan_docs.py
negative controls: missing row / duplicate row / unclosed fence / missing path rejected
markdown files checked: 155; errors: 0
```

V15 결과·독립 검토와 연결 문서를 포함했다. 실행·보안·제품 완성은 문서 구조 검사로 승인하지 않는다. 현재 범위는 [STATUS](../STATUS.md)와 [V15 결과](V15-filter-value-results.md)를 따른다. 아래 153개는 V14 당시 출력으로 보존한다.

## 2026-10-04 V14 이후 검사

```text
python3 tools/check_plan_docs.py
negative controls: missing row / duplicate row / unclosed fence / missing path rejected
markdown files checked: 153; errors: 0
```

V14 결과/독립 리뷰·V15 비교 계획과 후속 링크를 포함했다. 의미·보안·제품 승인은 이 문서 검사로 판정하지 않는다. 최신 실행 범위는 [STATUS](../STATUS.md)와 [V14 결과](V14-typed-apply-results.md)를 따른다. 아래 150개는 V13 당시 출력으로 보존한다.

## 2026-10-04 V13 이후 검사

```text
python3 tools/check_plan_docs.py
negative controls: missing row / duplicate row / unclosed fence / missing path rejected
markdown files checked: 150; errors: 0
```

V13 결과/독립 리뷰·V14 비교 계획과 후속 링크를 포함했다. 의미·보안·제품 승인은 이 문서 검사로 판정하지 않는다. 최신 실행 범위는 [STATUS](../STATUS.md)와 [V13 결과](V13-id-boundary-results.md)를 따른다. 아래 147개는 V12 당시 출력으로 보존한다.

## 2026-10-04 V12 이후 검사

명령: `python3 tools/check_plan_docs.py`

```text
negative controls: missing row / duplicate row / unclosed fence / missing path rejected
deliverables 5; questions 25; legacy 10; agendas 14; decisions 14; topics 13
markdown files checked: 147; errors: 0
```

V12 결과/독립 리뷰와 V13 비교 계획을 포함했다. 검사 한계는 아래와 같으며 의미·보안 판정으로 확대하지 않는다. 최신 실행 범위는 [STATUS](../STATUS.md)와 [V12 결과](V12-typed-transport-results.md)를 따른다. 다음 절의 140개는 V10 당시 출력이다.

## 2026-10-04 V10 이후 검사 범위

명령: `python3 tools/check_plan_docs.py`

```text
negative controls: missing row / duplicate row / unclosed fence / missing path rejected
deliverables 5; questions 25; legacy 10; agendas 14; decisions 14; topics 13
markdown files checked: 140; errors: 0
```

기존 고정 목록 외에 `alignment/*.md`와 `reviews/*.md`를 자동 포함한다. 임시 새 실험 문서에 없는 링크를 넣었을 때 이전 검사기는 성공했고, 확장한 검사기는 실패했다. 음성 대조 후 임시 파일을 삭제하고 위 명령을 다시 실행했다. 실험·리뷰가 늘면 검사 파일 수도 달라진다.

이 검사는 문서 구조와 링크 대상의 존재를 확인한다. 의미·보안·실행 보장은 각 실험 결과를 따른다. 최신 실행 범위는 [STATUS](../STATUS.md)와 [V10 결과](V10-session-recovery-results.md)에 있다.

## 2026-10-03 최초 실행 출력

명령: `python3 tools/check_plan_docs.py`

```text
negative controls: missing row / duplicate row / unclosed fence / missing path rejected
deliverables 5; questions 25; legacy 10; agendas 14; decisions 14; topics 13
markdown files checked: 49; errors: 0
```

이 출력은 문서 구조 검사 결과다. 아래 한계를 함께 적용한다.

> 2026-10-03. Validation Result이며 설계 승인·신규 runtime 검증과 다르다.

## 실행 명령과 범위

저장소 루트에서 `python3 tools/check_plan_docs.py`를 실행한다. [검사기](../../tools/check_plan_docs.py)는 결과물 A~E, 원문 §0~17·최초 7원칙, 질문 25개·Q0~Q9·14개 의제, DD 14개 머리/G 상태, topics 13개 현재 기준 링크, 선택한 Markdown의 파일 링크·code fence·archive 구획을 확인한다.

실패 대조: 정상 행/링크를 허용하고 누락 행·중복 행·닫히지 않은 fence/이력 구획·없는 링크를 거부하는지 확인한다. 대상 파일이나 표가 없으면 실패한다.

## 검증 한계

- 링크는 대상 파일 존재 검사다. 모든 Markdown 앵커나 에디터 렌더링을 검사하지 않는다.
- 표의 구조·상태 일치를 확인하며 자연어 의미 전체·보안·문법 실행은 검사하지 않는다.
- 의미 누락/분류 대조는 [독립 읽기 기록](../reviews/INTEGRATED-codex-luna-r1.md)에 있다.
- 최초 문서 검증 당시 신규 문법/SDK/planner/worker 실험은 미실행이었다. 이후 독립 spike의 실행 결과는 STATUS를 따른다.
- 문서 검사 명령 자체는 기존 workspace runtime 테스트를 실행하지 않는다.
- 커밋·푸시는 미실행이다. AIP와 상위 작업 경로에 Git 저장소가 없다. 초기 git 진단은 비-git 경로 보호 hook이 차단했으며, 이후 디렉터리 존재로 확인하고 git 명령을 반복하지 않았다.
