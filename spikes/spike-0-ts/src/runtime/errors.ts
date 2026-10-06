// Every failure leaving the runtime is one of these. `code` is the stable
// taxonomy key, `reason` is operation-specific (e.g. a `require ... else CODE`).

export interface AipErrorBody {
  code: string;
  operation: string;
  reason?: string;
  path?: string;
  message: string;
  retryable: boolean;
}

const STATUS: Record<string, number> = {
  'AIP.REQUEST.UNKNOWN_OPERATION': 404,
  'AIP.REQUEST.MALFORMED': 400,
  'AIP.INPUT.INVALID': 400,
  'AIP.INPUT.IDEMPOTENCY_KEY_REQUIRED': 400,
  'AIP.INPUT.IDEMPOTENCY_KEY_UNSUPPORTED': 400,
  'AIP.INPUT.REFERENCE_NOT_FOUND': 422,
  'AIP.INPUT.OUT_OF_RANGE': 422,
  'AIP.AUTH.UNAUTHENTICATED': 401,
  'AIP.AUTH.FORBIDDEN': 403,
  'AIP.NOT_FOUND': 404,
  'AIP.PRECONDITION.FAILED': 409,
  'AIP.INVARIANT.VIOLATED': 409,
  'AIP.IDEMPOTENCY.KEY_REUSED': 409,
  'AIP.CONCURRENCY.CONFLICT': 409,
  'AIP.INTERNAL': 500,
};

export const ERROR_CODES = Object.keys(STATUS);

export class AipError extends Error {
  body: AipErrorBody;
  constructor(code: string, operation: string, message: string, extra: { reason?: string; path?: string; retryable?: boolean } = {}) {
    super(message);
    this.body = { code, operation, message, retryable: extra.retryable ?? false, ...(extra.reason ? { reason: extra.reason } : {}), ...(extra.path ? { path: extra.path } : {}) };
  }

  get status(): number {
    return STATUS[this.body.code] ?? 500;
  }
}
