-- The original FaceReview app fills these values in Python (default=/onupdate=), so the tables carry no DB defaults.
-- AIP creates and transitions rely on `default database`, hence this one explicit DBA step before adoption.
-- created_at/updated_at are stored as UTC by the original (datetime.utcnow); the expression default keeps that.
ALTER TABLE `user` ALTER COLUMN created_at SET DEFAULT (UTC_TIMESTAMP()), ALTER COLUMN updated_at SET DEFAULT (UTC_TIMESTAMP()),
  ALTER COLUMN is_tutorial_done SET DEFAULT 0, ALTER COLUMN is_verify_email_done SET DEFAULT 0, ALTER COLUMN is_deleted SET DEFAULT 0;
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
