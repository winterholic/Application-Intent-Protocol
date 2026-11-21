# 계약 대조 재현 스크립트

`../contract.md`의 근거 명령이 쓰는 스크립트다. 저장소 루트에서 실행한다. 바이너리는 `AIP_BIN`(없으면 `$CARGO_TARGET_DIR/debug/aip`)을 쓴다. 전부 읽기 전용 검사이거나 임시 DB 스키마를 쓰는 호출이며 저장소 파일을 바꾸지 않는다.

| 파일 | 하는 일 |
| --- | --- |
| `run.py` | `aip service check`를 임시 파일로 돌리고 결과 코드를 모으는 공용 함수 |
| `syn1.py`, `syn2.py`, `syn4.py` | 문법 카드의 규칙 하나마다 정의 한 개를 만들어 수락·거절과 진단 코드를 확인한다. `DIFF`로 나오는 줄이 불일치 후보다 |
| `syn3.py` | 필드 타입 14종 × 필터 연산 10종의 수락 표 |
| `syn5.py` | 공개 count 경고와 `callerFilter` 정책 규칙 10건 |
| `codes.py` | 진단·wire·SDK 코드 레지스트리와 소스에서 실제로 쓰는 코드의 양방향 대조 |
| `doccodes.py` | 문서가 말하는 코드가 레지스트리에 있는지 |
| `links.py` | 문서 링크, 앵커, 인용한 파일 경로가 있는지 |
| `docblocks.py` | 문서의 ```aip 블록을 `check`에 넣어 본다(영구 시험은 `product/tests/doc-aip-examples.test.mjs`) |
| `http/app*/` | 서버를 띄워 wire를 확인하는 정의(`app.aip`)와 호출 스크립트 |

`http/` 스크립트는 한 번 쓰고 버리는 환경(전용 PostgreSQL 17, 포트 `18471`~`18475`, 스키마 `aip_hc*`, 개발 JWT)을 전제로 하며 지문·포트·경로가 코드에 박혀 있다. 다시 돌리려면 `aip service init/principal bind/gen/serve`를 한 뒤 `FP`와 포트를 바꿔 쓴다(절차는 [시작 가이드](../../product/GETTING-STARTED.md)와 같다).
