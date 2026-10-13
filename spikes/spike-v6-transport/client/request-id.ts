// 서버는 모든 응답 헤더 `x-request-id`에 요청 ID를 싣는다. 응답 본문 계약은 바꾸지 않으므로 ID는 본문이 아니라
// 헤더에서 읽어 파싱된 응답 객체에 연결해 두고, 실패 객체(던져지는 오류, 실패 결과)에 `requestId`로 옮긴다.
// 실패를 신고할 때 이 값을 함께 주면 운영자가 같은 ID의 서버 로그 줄을 찾는다.
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
  return body !== null && typeof body === "object" ? ids.get(body) : undefined;
}

/** 응답 객체에 연결된 ID가 있을 때만 `requestId` 필드를 만든다. */
export function requestIdField(body: unknown): { requestId?: string } {
  const requestId = requestIdOf(body);
  return requestId === undefined ? {} : { requestId };
}

export type RequestFailure = Error & { code: string; requestId?: string };

/** 서버 응답(`response`)에서 비롯된 실패. 응답이 요청 ID를 가졌으면 `requestId`를 싣는다. */
export function failure(code: string, message: string, response?: unknown): RequestFailure {
  return Object.assign(new Error(message), { code }, requestIdField(response));
}
