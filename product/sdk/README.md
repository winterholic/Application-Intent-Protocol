# AIP TypeScript SDK

TypeScript caller SDK for [Application Intent Protocol](https://github.com/winterholic/Application-Intent-Protocol) services.

Application Intent Protocol(AIP) 서비스를 호출하는 TypeScript SDK입니다. 서버가 최종 권한을 가지며, 이 SDK는 배포된 계약이 노출한 읽기, 전이, 묶음 쓰기, operation, job만 표현합니다.

## 설치

패키지 이름과 설치 경로는 릴리스 노트의 설치 안내(`product/INSTALL.md`)를 따릅니다. 1.0.0 공개 전에는 이름이 잠정입니다.

```sh
npm install @aip/sdk
```

## 사용

SDK는 `aip service gen`이 만든 계약 binding과 함께 씁니다. binding의 계약 지문(`contractFingerprint`)이 배포된 서비스와 다르면 서버가 `WIRE.CONTRACT_MISMATCH`로 거절합니다. 배포가 바뀌면 binding을 다시 생성하십시오.

```ts
import { connect } from "@aip/sdk";
import { contract } from "./binding.js";

const client = connect("https://api.example.com", accessToken, contract);
```

전체 설명은 저장소의 `product/SDK.md`를 보십시오.

## 호환

이 패키지가 말하는 protocol은 `package.json`의 `aip.protocol` 필드에 있습니다. 엔진, SDK, protocol 버전의 대응은 저장소의 `COMPATIBILITY.md`가 정본입니다.

## 라이선스

MIT
