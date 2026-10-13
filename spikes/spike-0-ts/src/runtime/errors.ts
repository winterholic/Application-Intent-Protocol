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
  'POC.REQUEST.UNKNOWN_OPERATION': 404,
  'POC.REQUEST.MALFORMED': 400,
  'POC.INPUT.INVALID': 400,
  'POC.INPUT.IDEMPOTENCY_KEY_REQUIRED': 400,
  'POC.INPUT.IDEMPOTENCY_KEY_UNSUPPORTED': 400,
  'POC.INPUT.REFERENCE_NOT_FOUND': 422,
  'POC.INPUT.OUT_OF_RANGE': 422,
  'POC.AUTH.UNAUTHENTICATED': 401,
  'POC.AUTH.FORBIDDEN': 403,
  'POC.NOT_FOUND': 404,
  'POC.PRECONDITION.FAILED': 409,
  'POC.INVARIANT.VIOLATED': 409,
  'POC.IDEMPOTENCY.KEY_REUSED': 409,
  'POC.CONCURRENCY.CONFLICT': 409,
  'POC.INTERNAL': 500,
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
