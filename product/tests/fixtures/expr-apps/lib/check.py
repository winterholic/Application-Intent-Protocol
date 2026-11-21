"""시나리오 공통 도구. 결과는 `RESULT PASS|FAIL <이름> :: <상세>` 줄로 runner에 전달한다."""
import json
import os
import subprocess
import sys

ADDR = os.environ["AIP_ADDR"]
BASE_URL = f"http://{ADDR}"
TOKENS = json.loads(os.environ["AIP_TOKENS"])
SCHEMA = os.environ["AIP_SCHEMA"]
FINGERPRINT = os.environ["AIP_FINGERPRINT"]


def sql(query: str) -> str:
    """서비스를 거치지 않고 PostgreSQL을 직접 읽어 SDK 결과를 교차 확인한다."""
    r = subprocess.run(["psql", os.environ["AIP_DB"], "-X", "-v", "ON_ERROR_STOP=1", "-At", "-c", query], capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"psql 실패: {r.stderr}")
    return r.stdout.strip()


def scenario(name, body):
    try:
        body()
        print(f"RESULT PASS {name}")
    except BaseException as error:  # AssertionError와 AipError 모두 FAIL로 기록한다
        print(f"RESULT FAIL {name} :: {' '.join(str(error).split())[:400]}")
    sys.stdout.flush()
