# BullMQ 큐와 AIP job 대응 (WBS 7.3.3.5, 2026-10-18 정정판)

이전판은 MIT 큐 16개를 `operation job` 16개로 1:1 선언했다고 적었다. 실제로는 입력이 전부 `projectId: Text` 하나인 자리표시자였고 구현 함수가 없어서 enqueue하면 `WIRE.EXTENSION_ERROR`로 실패했다(**재현**, `porting/infisical/evidence/before-legacy-53b1537.txt`). 또 큐 수도 틀렸다. `QueueName`은 54개이고 MIT 범위로 읽히는 것은 33개 안팎이다(`mit` 30 + `unreferenced` 3, 분류 정의는 [INFISICAL-SELECTION.md](INFISICAL-SELECTION.md)의 BullMQ 절).

## 현재 상태

| 상태 | 큐 수 | 뜻 |
|---|---|---|
| 부분 구현 | 1 | `app.aip`에 선언돼 있고 worker 함수가 있으며 실DB로 실행했다 |
| 미선언 | 53 | `app.aip`에 선언이 없다. 선언만 있고 구현이 없는 job은 만들지 않는다(실패할 endpoint를 서비스하지 않기 위해) |

선언했다고 해서 동작하는 것이 아니다. 이 표는 "무엇이 있고 무엇이 없는가"를 구분한다.

### 부분 구현: `secret-reminder` → `operation job secretReminder`

- 입력 `secret: Secret.Id`. enqueue 시점의 `allow`가 호출자가 그 시크릿을 읽을 수 있는 프로젝트 구성원인지 확인한다(원본 큐 payload 모양은 아직 대조하지 않았다).
- worker `infisical_jobs.secret_reminder`가 job 주체 권한으로 `Secret`을 읽고 공개 전이 `Secret.remind`를 적용한다. 전이는 `remindedAt`을 갱신하고 소유자에게 outbox 알림(`notify`)을 남긴다.
- **이것은 알림 기록이지 발송이 아니다.** 메일·Slack 전송, 수신자 계산, 반복 일정은 구현하지 않았다. 전송 증명은 전송 receipt이며 outbox 기록이나 job 성공은 메일 발송이 아니다.
- **동작 확인**(실제 PostgreSQL 17): 읽기 가능한 구성원 enqueue 후 `SUCCEEDED`, 외부인·무권한 구성원은 enqueue 거부, outbox 행 생성, 감사 행(`via: job`). `tests/test_live_db.py`의 `test_reminder_job_*`와 `evidence/baseline-9-sdk.txt`, `baseline-10-*.txt`.

## 큐 54개 전체표

분류는 `QueueName.<이름>`을 참조하는 파일 위치로 정한 것이고 라이선스 판정이 아니다. "계획"은 AIP에서 맡을 수단의 초안이며 확정이 아니다.

| 큐 | 분류 | 계획(초안) | 상태 |
|---|---|---|---|
| `secret-reminder` | mit | operation job + 전이 `notify` | 부분 구현 |
| `audit-log-prune` | unreferenced | 원본이 반복 큐 정리용으로 enum만 남김. 포팅 안 함 | 미선언 |
| `pki-alert-v2-event` | mit | 이벤트 job | 미선언 |
| `pki-subscriber` | mit | 인증서 구독 발급 job (G6) | 미선언 |
| `sync-integrations` | mit | integration 동기화 job (G8) | 미선언 |
| `secret-webhook` | mixed | 웹훅 전송 job + 등록된 outbound 전송 capability | 미선언 |
| `secret-full-repo-scan` | unreferenced | 미사용 | 미선언 |
| `secret-push-event-scan` | unreferenced | 미사용 | 미선언 |
| `upgrade-project-to-ghost` | mit | V3 이전 마이그레이션. 레거시 | 미선언 |
| `dynamic-secret-revocation` | mixed | 동적 시크릿은 EE | 미선언 |
| `dynamic-secret-lease-revocation-failed-email` | ee | EE | 미선언 |
| `ca-lifecycle` | mit | CA 수명주기 job (G6) | 미선언 |
| `certificate-issuance` | mit | 인증서 발급 job (G6) | 미선언 |
| `secret-replication` | mixed | 폴더 간 복제. 값 복제는 G2 대기 | 미선언 |
| `secret-sync` | mit | 외부 대상 동기화 job. 값은 G2 대기 | 미선언 |
| `pki-sync` | mit | PKI 동기화 job | 미선언 |
| `pki-sync-health-check` | mit | 상태 점검 job | 미선언 |
| `project-v3-migration` | mit | 레거시 마이그레이션 | 미선언 |
| `access-token-status-update` | mit | 토큰 상태 갱신. 원본 의미 대조 전 | 미선언 |
| `import-secrets-from-external-source` | mit | 외부 가져오기. 값은 G2 대기 | 미선언 |
| `vault-import-side-effects` | mit | 가져오기 부수 효과 | 미선언 |
| `app-connection-secret-sync` | mit | 연결 기반 동기화 | 미선언 |
| `secret-rotation-v2` | ee | EE | 미선언 |
| `secret-rotation-v2-rotate-secrets` | ee | EE | 미선언 |
| `pam-credential-rotation` | ee | EE | 미선언 |
| `pam-credential-rotation-rotate` | ee | EE | 미선언 |
| `pam-heartbeat` | ee | EE | 미선언 |
| `pam-heartbeat-check` | ee | EE | 미선언 |
| `folder-tree-checkpoint` | mit | 폴더 트리 스냅샷 job | 미선언 |
| `invalidate-cache` | mit | AIP에는 앱 캐시가 없다. 대응 불필요일 수 있음 | 미선언 |
| `secret-scanning-v2` | ee | EE | 미선언 |
| `secret-scanning-v2-full-scan` | ee | EE | 미선언 |
| `secret-scanning-v2-realtime-scan` | ee | EE | 미선언 |
| `user-notification` | mit | 전이 `notify` + outbox | 미선언 |
| `alert-dispatch` | mit | 알림 전송 job | 미선언 |
| `event-outbox-flush` | mit | AIP 내장 outbox 릴레이가 대응할 가능성. 대조 전 | 미선언 |
| `audit-report-generation` | ee | EE | 미선언 |
| `pam-session-expiration` | mixed | PAM은 EE | 미선언 |
| `pam-discovery-scan` | ee | EE | 미선언 |
| `pki-acme-challenge-validation` | ee | EE | 미선언 |
| `pki-discovery-scan` | ee | EE | 미선언 |
| `app-connection-credential-rotation` | mit | 연결 자격 회전 job. 값은 G2 대기 | 미선언 |
| `app-connection-credential-rotation-rotate` | mit | 위와 같음 | 미선언 |
| `audit-log-clickhouse-batch` | ee | EE | 미선언 |
| `audit-log-stream-outbox` | ee | EE | 미선언 |
| `audit-log-collapsed-flush` | ee | EE | 미선언 |
| `ca-auto-renewal` | mit | 인증서 갱신 job (G6) | 미선언 |
| `project-hard-delete` | mit | 소프트 삭제 후 정리. G10 | 미선언 |
| `project-env-hard-delete` | mit | 위와 같음 | 미선언 |
| `signer-auto-renewal` | mit | 서명자 갱신 job | 미선언 |
| `secret-blind-index-migration` | mit | blind index 이전. G4 | 미선언 |
| `usage-event` | mit | 사용량 이벤트. 텔레메트리 | 미선언 |
| `integration-deprecation-notice` | mit | 알림 job | 미선언 |
| `legacy-pki-deprecation-notice` | mit | 알림 job | 미선언 |

이전판 16개 이름과의 관계: 이전판의 `secretWebhookDispatch`, `integrationSync`, `appConnectionSecretSync`, `secretSyncDispatch`, `pkiSyncDispatch`, `pkiSyncHealthCheck`, `folderTreeCheckpoint`, `importSecretsFromExternalSource`, `invalidateCache`, `userNotification`, `alertDispatch`, `eventOutboxFlush`, `accessTokenStatusUpdate`, `upgradeProjectToGhost`, `auditLogPrune`은 선언을 모두 제거했다. 입력 모양이 원본 payload와 대조된 적이 없고 구현이 없었기 때문이다.
