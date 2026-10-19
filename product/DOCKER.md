# 네이티브 서비스와 선택형 Docker 실행

앱 패키지에는 `deploy/Dockerfile`, `deploy/compose.yaml`, `deploy/aip-app.service`, `.dockerignore`가 포함된다. 파일을 설치하거나 원격 서버를 바꾸는 동작은 자동 실행하지 않는다. 기본 경로는 [네이티브 설치·기동](APP-DEPLOY.md)이며 Docker는 Linux 앱 패키지를 사용하는 선택지다.

## Linux Docker 이미지

Linux GNU arm64/x64 서버에서 같은 OS/CPU로 생성한 패키지를 준비한다. macOS binary를 Docker 이미지에 복사해 Linux binary로 바꾸지는 못한다. `docker build` 중 실제 `service check`로 플랫폼·실행 파일·파일 무결성을 검사한다.

```sh
cd /srv/my-app/1.0.0
node tools/app-deploy.mjs check
docker build --file deploy/Dockerfile --tag my-app:1.0.0 .
docker image inspect my-app:1.0.0 --format '{{.Id}}'
```

**빌드 context는 비밀 없는 앱 패키지 하나여야 한다.** 운영 디렉터리나 checkout을 context로 쓰지 않는다. `.dockerignore`도 일반 비밀 파일과 캐시를 제외한다. 이미지에는 Rust/compiler·소스 checkout·DB·운영 credential을 포함하지 않는다. Node official image를 digest로 고정하고 Python·Bubblewrap·CA를 설치한다. OS 패키지 설치 목록은 이미지의 `/usr/share/aip-runtime/packages.txt`에 남는다. 앱 입력·base digest·Dockerfile은 고정하지만 apt repository는 움직이므로 바이트 단위 재현 이미지를 보장하지 않는다. 반복 배포는 빌드한 최종 image digest를 고정해 사용한다. 더 엄격한 재빌드는 조직의 고정 package mirror/완성 runtime image 정책을 적용해야 한다.

이미지의 기본 사용자는 UID/GID `10001:10001`이다. `serve`는 foreground이고 `HEALTHCHECK`는 제품 `/ready`를 검사한다. 기본 DB 주소·schema·비밀은 없다. 외부 설정 디렉터리를 `/run/aip`에 읽기 전용으로 mount하며 `service.json`과 공개 JWKS는 UID 10001이 읽을 수 있어야 한다. 비밀은 secret manager 또는 runtime 환경으로 주입한다. 개인키는 넣지 않는다.

## 실행과 종료

```sh
# service.json은 listen=127.0.0.1:8080, database_url_env=APP_RUNTIME_DATABASE_URL 등을 명시한다.
# 필요한 DSN은 사전에 환경으로 주입한다.
export AIP_APP_IMAGE=my-app:1.0.0
export AIP_OPERATOR_DIRECTORY=/srv/my-app/operator
docker compose -f deploy/compose.yaml config --quiet
docker compose -f deploy/compose.yaml up -d
docker compose -f deploy/compose.yaml ps
docker compose -f deploy/compose.yaml stop
```

Compose 예제는 **Linux Docker Engine의 host network**를 쓴다. 엔진은 loopback에만 bind하므로 일반 bridge `ports:`만으로 접근할 수 없다. host network는 호스트 네트워크 namespace를 공유한다. 공개 HTTPS는 해당 호스트의 별도 TLS reverse proxy가 담당한다. Docker Desktop의 host-network 기능 활성화나 bridge용 프록시 sidecar는 이 예제의 검증 범위가 아니다. [Docker host network 문서](https://docs.docker.com/engine/network/drivers/host/).

read-only rootfs, 모든 capability 제거, no-new-privileges, 제한된 `/tmp` tmpfs, init과 SIGTERM 종료, 메모리·PID·로그 보존 한도를 지정했다. 512MiB/128 PID는 예시 한도이며 실제 workload 측정 후 조정한다. 사용자 DB는 외부 상태다. 객체 파일 저장소를 쓰는 앱은 별도 전용 영속 볼륨과 백업이 필요하며, 해당 경로를 실제 운영 설정에 연결해야 한다. [Docker run 문서](https://docs.docker.com/reference/cli/docker/container/run/).

이미지 재생성 전후 외부 DB·설정이 보존되는지 검증한다. schema init/migration은 앱 실행 때 자동 적용하지 않는다. 네이티브 명령이나 별도 일회성 컨테이너에서 검토한 digest와 migration 역할 환경을 사용하고, migration 역할 비밀을 상시 serving 컨테이너에 넣지 않는다.

## worker 격리 경계

worker 앱은 `workers.directory=/opt/aip/app/<manifest extensions.directory>`와 올바른 언어·쓰기 권한을 설정한다. Node/Python의 추가 의존성은 별도로 준비한다. Bubblewrap 도구 설치와 실제 격리 실행 가능 여부는 다르다. 기본 Docker seccomp·user namespace·AppArmor 정책에 따라 내부 Bubblewrap이 거절될 수 있다.

그 경우 worker 컨테이너 배포는 지원 조건 미충족으로 남긴다. `privileged`, `seccomp=unconfined`, `isolation=none`으로 바꿔 통과시키지 않는다. 검증된 네이티브 Linux/Bubblewrap 경로를 사용하거나 기존 허용 정책에서 실제 격리 검사를 통과한 환경을 준비한다. 도구가 존재하거나 `/ready`가 200인 것만으로 worker 실행 성공을 주장하지 않는다. 기본 Docker 구성의 worker 실행은 CI에서 별도 양성 실행 증거가 나오기 전까지 미검증이다.

## systemd 네이티브 실행

`deploy/aip-app.service`는 `/srv/my-app/1.0.0`, 외부 operator 경로, `/usr/bin/node`, 전용 사용자 `aip-app`을 가정한 편집용 예제다. 운영자가 경로·소유권·환경 파일 권한을 맞춘 뒤 검토해 설치한다. `ExecStartPre`는 사전 검사, `ExecStart`는 foreground supervisor를 실행한다. `KillMode=mixed`는 먼저 supervisor에 SIGTERM을 전달하고 제한 시간 초과 시 unit cgroup을 회수한다. 이 Mac에서 systemd 설치·서비스 실행은 하지 않는다. Linux 실제 unit 등록·재부팅 복구는 별도 운영 환경에서 확인해야 한다.

변경배포는 새 package 설치 → 백업·계획 검토 → 기존 서비스 종료 → 정확한 digest migration → unit의 버전 경로와 worker 경로 갱신 → 재기동·ready 확인 순서다. 이전 실행 파일 선택과 DB 복원은 다르며 자세한 한계는 [변경배포 가이드](APP-DEPLOY.md#변경배포)를 따른다.

## 자동 검증 범위

`AIP_TEST_APP_DOCKER=1 node --test product/tests/app-deploy-live.test.mjs`는 Linux에서 실제 패키지로 image를 만들고, 비root/read-only/cap-drop 설정에서 인증 조회·ready·SIGTERM 종료·재생성·외부 DB/공개 JWKS 보존을 검사한다. 현재 macOS 실행은 native 검증만 수행하며 Docker image 단계는 `NOT RUN`으로 출력한다. 최종 지원 판정에는 Linux CI 원문 결과가 필요하다.

기반 runtime 출처: [Node official image](https://github.com/nodejs/docker-node). Linux arm64 Docker 실행과 운영 서버 배포는 별도 실행 증거가 필요하다.
