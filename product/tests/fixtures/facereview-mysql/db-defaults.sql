-- The original FaceReview app fills these values in Python (default=/onupdate=), so the tables carry no DB defaults.
-- AIP creates and transitions rely on `default database`, hence this one explicit DBA step before adoption.
-- created_at/updated_at are stored as UTC by the original (datetime.utcnow); the expression default keeps that.
ALTER TABLE `user` ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP()), ALTER COLUMN updated_at SET DEFAULT (UTC_TIMESTAMP()),
  ALTER COLUMN is_tutorial_done SET DEFAULT 0, ALTER COLUMN is_verify_email_done SET DEFAULT 0, ALTER COLUMN is_deleted SET DEFAULT 0;
-- view_count: the original never writes NULL (default=0), and the `view` transition counts with `view_count = view_count + 1`, which the
-- definition language allows only on a non-null Int. Backfill stray NULLs (the original fails on them with a 500) and make the column NOT NULL.
UPDATE video SET view_count = 0 WHERE view_count IS NULL;
ALTER TABLE video MODIFY view_count bigint NOT NULL DEFAULT 0 COMMENT '조회수';
ALTER TABLE video ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP()), ALTER COLUMN updated_at SET DEFAULT (UTC_TIMESTAMP()),
  ALTER COLUMN is_deleted SET DEFAULT 0, ALTER COLUMN view_count SET DEFAULT 0, ALTER COLUMN duration SET DEFAULT 0;
ALTER TABLE comment ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP()), ALTER COLUMN updated_at SET DEFAULT (UTC_TIMESTAMP()),
  ALTER COLUMN is_modified SET DEFAULT 0, ALTER COLUMN is_deleted SET DEFAULT 0;
ALTER TABLE video_like ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP());
ALTER TABLE video_bookmark ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP());
ALTER TABLE video_request ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP()), ALTER COLUMN updated_at SET DEFAULT (UTC_TIMESTAMP()),
  ALTER COLUMN status SET DEFAULT 'PENDING';
ALTER TABLE video_view_log ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP()), ALTER COLUMN updated_at SET DEFAULT (UTC_TIMESTAMP());
ALTER TABLE user_favorite_genre ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP()), ALTER COLUMN updated_at SET DEFAULT (UTC_TIMESTAMP());
ALTER TABLE user_point_history ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP()), ALTER COLUMN updated_at SET DEFAULT (UTC_TIMESTAMP());
