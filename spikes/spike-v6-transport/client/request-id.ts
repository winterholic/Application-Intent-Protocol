// 서버는 모든 응답 헤더 `x-request-id`에 요청 ID를 싣고, 실패 본문에도 같은 값을 `requestId`로, 재시도 지침을
// `retry`(never·later·check-first·reauthenticate)로 싣는다. 본문 값이 있으면 그것을 쓰고, 없으면(옛 서버, 프록시가
// 만든 응답) 헤더에서 읽어 파싱된 응답 객체에 연결해 둔 값을 쓴다. 실패 객체(던져지는 오류, 실패 결과)에 `requestId`와
// `retry`로 옮긴다. 실패를 신고할 때 이 값을 함께 주면 운영자가 같은 ID의 서버 로그 줄을 찾는다.
const SAFE_ID = /^[A-Za-z0-9._-]{8,64}$/;
const ids = new WeakMap<object, string>();

/** 응답 헤더에서 읽은 ID를 파싱된 본문 객체에 연결한다. 형식이 맞지 않는 값은 버린다. */
export function rememberRequestId(body: unknown, response: unknown): void {
  if (body === null || typeof body !== "object") return;
  const headers = (response as { headers?: { get?: (name: string) => string | null } } | null)?.headers;
  const id = headers?.get?.("x-request-id");
  if (typeof id === "string" && SAFE_ID.test(id)) ids.set(body, id);
}

export function requestIdOf(body: unknown): string | undefined {
  if (body === null || typeof body !== "object") return undefined;
  const inBody = (body as { requestId?: unknown }).requestId;
  if (typeof inBody === "string" && SAFE_ID.test(inBody)) return inBody;
  return ids.get(body);
}

import type { RetryGuidance } from "../../spike-v5-sdk/sdk/generic.ts";
export type { RetryGuidance } from "../../spike-v5-sdk/sdk/generic.ts";
const GUIDANCE = new Set<string>(["never", "later", "check-first", "reauthenticate"]);

/** 실패 본문의 재시도 지침. 레지스트리에 없는 값은 믿지 않는다. */
export function retryOf(body: unknown): RetryGuidance | undefined {
  if (body === null || typeof body !== "object") return undefined;
  const retry = (body as { retry?: unknown }).retry;
  return typeof retry === "string" && GUIDANCE.has(retry) ? (retry as RetryGuidance) : undefined;
}

/** 응답 객체에 연결된 ID가 있을 때만 `requestId` 필드를 만든다. */
export function requestIdField(body: unknown): { requestId?: string } {
  const requestId = requestIdOf(body);
  return requestId === undefined ? {} : { requestId };
}

export type RequestFailure = Error & { code: string; requestId?: string; retry?: RetryGuidance };

/** 서버 응답(`response`)에서 비롯된 실패. 응답이 요청 ID와 재시도 지침을 가졌으면 `requestId`와 `retry`를 싣는다. */
export function failure(code: string, message: string, response?: unknown): RequestFailure {
  const retry = retryOf(response);
  return Object.assign(new Error(message), { code }, requestIdField(response), retry === undefined ? {} : { retry });
}
