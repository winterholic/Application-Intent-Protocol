-- Slice13-C DBA step (run after db-defaults.sql). The original Video.view_count column is nullable (BigInteger, default=0),
-- but the AIP language only allows `view_count = view_count + 1` on a non-null Int, and adoption rejects a declaration that
-- is stricter than the live column ("nullable 불일치"). The original service itself fails on NULL (None += 1), so rows
-- written by the original app never hold NULL; this makes the schema say so.
UPDATE video SET view_count = 0 WHERE view_count IS NULL;
ALTER TABLE video MODIFY view_count BIGINT NOT NULL DEFAULT 0;
