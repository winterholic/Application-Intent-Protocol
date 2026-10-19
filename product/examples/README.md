# 패키징 예제 두 개

`event-checkin`은 정수 Id와 본인 이벤트의 체크 전이, `folder-notes`는 UUID와 본인 폴더 이름·순서 patch를 사용한다. 둘 다 외부 worker·추가 의존성이 없는 독립 앱 입력이다. 로그인 UI와 사용자를 만드는 API는 포함하지 않으며 actor 행과 principal은 운영자가 준비한다.

checkout root에서 SDK 빌드 도구를 한 번 설치하고 **존재하지 않는 앱 밖 출력 경로**를 지정한다.

```sh
npm ci --prefix spikes/spike-0-ts --ignore-scripts --no-audit --no-fund
node product/tools/package.mjs --manifest product/examples/event-checkin --out /absolute/new-event-bundle
node product/tools/package.mjs --manifest product/examples/folder-notes --out /absolute/new-folder-bundle
```

각 디렉터리를 대상 서버로 옮긴 뒤 패키지 안의 `tools/app-deploy.mjs install --out <새설치경로>`를 실행한다. [외부 설정·init·serve·ready·변경배포](../APP-DEPLOY.md)와 [Docker/systemd 예제](../DOCKER.md)를 따른다. SDK는 각 패키지의 `sdk/aip-sdk-0.1.0.tgz`, 생성 binding은 `bindings.ts`다.

Node/Python worker를 사용하는 패키지와 실제 실행 예시는 `app-package-e2e.test.mjs`, `app-deploy-live.test.mjs`가 임시 입력부터 생성해 검사한다. 두 입력·중간 bundle은 검사 종료 시 회수한다.
