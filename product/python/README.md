# AIP Python SDK

Python caller SDK for [Application Intent Protocol](https://github.com/winterholic/Application-Intent-Protocol) services.

Application Intent Protocol(AIP) 서비스를 호출하는 Python SDK입니다. 표준 라이브러리만 쓰며 Python 3.9 이상에서 동작합니다. 서버가 최종 권한을 가지고, 이 SDK는 배포된 계약이 노출한 읽기, 전이, 묶음 쓰기, operation, job만 표현합니다.

## 설치

배포 이름과 import 이름은 다릅니다. 1.0.0 공개 전에는 배포 이름이 잠정입니다(설치 안내는 저장소의 `product/INSTALL.md`).

```sh
pip install application-intent-protocol-sdk
```

```python
import aip_sdk  # import 이름은 배포 이름과 무관하게 aip_sdk
```

## 사용

```python
from aip_sdk import AipError, WriteUnsettled, connect

# 생성 binding의 `export const contractFingerprint = "..."` 값을 고정하면 배포가 바뀐 것을 감지한다.
client = connect("https://api.example.com", access_token, contract_fingerprint)
rows = client.read({"read": "Event", "select": ["id", "checked"]})
```

전체 설명은 저장소의 `product/SDK.md`를 보십시오.

## 호환

`aip_sdk.PROTOCOL`이 이 SDK가 말하는 protocol입니다. 엔진, SDK, protocol 버전의 대응은 저장소의 `COMPATIBILITY.md`가 정본입니다.

## 라이선스

MIT
