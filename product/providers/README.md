# Google Cloud Storage 서버 어댑터

제품의 `objects.kind:"gcs_hmac"`는 Rust에서 Google XML API를 사용하는 HMAC backend다. 아래 Node/Python 어댑터는 공식 SDK와 Application Default Credentials(ADC)를 쓰는 **별도의 서버 확장 경로**다. 제품 설정의 자동 backend 선택 항목은 아니며, 신뢰한 서버 bridge에서 호출한다. network-deny worker 안에서 직접 실행하지 않는다.

배포할 서버 프로젝트에서 필요한 패키지만 설치한다.

```sh
npm install @google-cloud/storage
# 또는 Python 서버의 가상환경에서
python -m pip install google-cloud-storage
```

Node 예제는 실제 제공 API다. `owner`는 인증된 서버 신원에서 얻고 클라이언트 입력을 그대로 사용하지 않는다.

```js
import { randomUUID } from "node:crypto";
import { createGoogleStorageProvider } from "./google-storage.mjs";
const provider = await createGoogleStorageProvider({bucket:"app-files"});
const storage = provider.forOwner(verifiedPrincipal);
const id = randomUUID();
await storage.upload(id, binaryReadable, {size:byteLength, contentType:"application/octet-stream"});
await storage.download(id, binaryWritable);
```

Python은 `create_google_storage_provider(bucket="app-files").for_owner(verified_principal)`로 만들고 `upload(id, source, size=..., content_type=...)`, `download(id, destination)`, `head(id)`, `delete(id)`를 사용한다. 동기 API이므로 async 서버에서는 제한된 thread executor나 별도 작업 queue로 실행한다.

SDK의 기본 인증 체인이 attached service identity·WIF·로컬 ADC를 처리한다. credential 파일·private key·access token을 프론트에 전달하지 않는다. 배포 identity에 필요한 bucket 권한을 별도로 부여해야 한다. SDK 패키지 버전은 소비자 프로젝트 lockfile로 고정한다.

두 어댑터는 owner namespace, UUID object ID, 업로드 크기, resumable upload, CRC32C 검사와 generation 조건을 사용한다. 업로드는 `ifGenerationMatch=0`으로 덮어쓰기를 거부하며, 다운로드와 삭제는 조회한 generation에 한정한다. 요청별 timeout을 적용하지만 스트림 입력 자체가 멈추는 경우 전체 작업 수명은 호출한 서버에서 제한해야 한다. owner별 누적 quota·비용 예산·미완료 업로드 수명 주기는 bucket 운영 정책의 책임이다.

검증은 주입한 SDK client로 API 호출·크기·namespace·generation 동작을 확인한다. 실제 Google SDK 인증·권한·네트워크 실행은 GCP 자격증명이 없어 미실행이다. [Google 공식 Node SDK](https://cloud.google.com/nodejs/docs/reference/storage/latest), [Python SDK](https://cloud.google.com/python/docs/reference/storage/latest), [ADC](https://docs.cloud.google.com/docs/authentication/application-default-credentials), [HMAC XML 호환 경로](https://docs.cloud.google.com/storage/docs/aws-simple-migration)를 참고한다. 다른 프로젝트 코드를 복사하지 않았다. 선택적 공식 SDK의 라이선스는 배포 시 사용하는 버전과 전이 의존성을 함께 확인한다.
