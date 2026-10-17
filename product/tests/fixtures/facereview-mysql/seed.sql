INSERT INTO `user` (user_id, email, password, name, role, profile_image_id, is_tutorial_done, is_verify_email_done, is_deleted, created_at, updated_at) VALUES
 ('11111111-1111-4111-8111-111111111111', 'a@example.com', 'pbkdf2:hash-a', '앨리스', 'GENERAL', 3, 1, 1, 0, '2026-10-01 00:00:00', '2026-10-01 00:00:00'),
 ('22222222-2222-4222-8222-222222222222', 'b@example.com', 'pbkdf2:hash-b', '밥', 'ADMIN', NULL, 0, 1, 0, '2026-10-02 00:00:00', '2026-10-02 00:00:00');
INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at) VALUES
 ('aaaaaaaa-0000-4000-8000-000000000001', 'dQw4w9WgXcQ', '고양이 영상', '냥채널', 'animal', 212, 10, 0, '2026-10-03 10:00:00', '2026-10-03 10:00:00'),
 ('aaaaaaaa-0000-4000-8000-000000000002', 'abcdefghijk', '요리 영상 고양이', '쿡채널', 'cook', 300, 5, 0, '2026-10-04 10:00:00', '2026-10-04 10:00:00'),
 ('aaaaaaaa-0000-4000-8000-000000000003', 'zzzzzzzzzzz', '삭제된 영상', '냥채널', 'etc', 1, 0, 1, '2026-10-05 10:00:00', '2026-10-05 10:00:00'),
 ('aaaaaaaa-0000-4000-8000-000000000004', 'nullflagvid', '플래그 NULL 영상', NULL, 'vlog', NULL, NULL, NULL, '2026-10-06 10:00:00', '2026-10-06 10:00:00');
INSERT INTO comment (comment_id, video_id, user_id, content, is_modified, is_deleted, created_at, updated_at) VALUES
 ('cccccccc-0000-4000-8000-000000000001', 'aaaaaaaa-0000-4000-8000-000000000001', '11111111-1111-4111-8111-111111111111', '첫 댓글', 0, 0, '2026-10-07 10:00:00', '2026-10-07 10:00:00'),
 ('cccccccc-0000-4000-8000-000000000002', 'aaaaaaaa-0000-4000-8000-000000000001', '22222222-2222-4222-8222-222222222222', '두번째 댓글', 1, 0, '2026-10-08 10:00:00', '2026-10-08 11:00:00'),
 ('cccccccc-0000-4000-8000-000000000003', 'aaaaaaaa-0000-4000-8000-000000000001', '11111111-1111-4111-8111-111111111111', '지워진 댓글', 0, 1, '2026-10-09 10:00:00', '2026-10-09 10:00:00'),
 ('cccccccc-0000-4000-8000-000000000004', 'aaaaaaaa-0000-4000-8000-000000000003', '11111111-1111-4111-8111-111111111111', '삭제된 영상의 댓글', 0, 0, '2026-10-10 10:00:00', '2026-10-10 10:00:00');
INSERT INTO video_like (video_like_id, video_id, user_id, created_at) VALUES
 ('dddddddd-0000-4000-8000-000000000001', 'aaaaaaaa-0000-4000-8000-000000000001', '11111111-1111-4111-8111-111111111111', '2026-10-11 10:00:00'),
 ('dddddddd-0000-4000-8000-000000000002', 'aaaaaaaa-0000-4000-8000-000000000002', '22222222-2222-4222-8222-222222222222', '2026-10-11 11:00:00');
INSERT INTO video_bookmark (video_bookmark_id, video_id, user_id, created_at) VALUES
 ('eeeeeeee-0000-4000-8000-000000000001', 'aaaaaaaa-0000-4000-8000-000000000002', '11111111-1111-4111-8111-111111111111', '2026-10-12 10:00:00');
INSERT INTO video_request (video_request_id, user_id, youtube_url, youtube_full_url, status, admin_comment, created_at, updated_at) VALUES
 ('ffffffff-0000-4000-8000-000000000001', '11111111-1111-4111-8111-111111111111', 'reqvideo001', 'https://www.youtube.com/watch?v=reqvideo001', 'PENDING', NULL, '2026-10-13 10:00:00', '2026-10-13 10:00:00');
INSERT INTO video_view_log (video_view_log_id, video_id, user_id, guest_token, created_at, updated_at) VALUES
 ('99999999-0000-4000-8000-000000000001', 'aaaaaaaa-0000-4000-8000-000000000001', '11111111-1111-4111-8111-111111111111', NULL, '2026-10-14 10:00:00', '2026-10-14 10:00:00'),
 ('99999999-0000-4000-8000-000000000002', 'aaaaaaaa-0000-4000-8000-000000000001', NULL, 'guest-xyz', '2026-10-14 11:00:00', '2026-10-14 11:00:00');
INSERT INTO user_favorite_genre (user_favorite_genre_id, user_id, genre, created_at, updated_at) VALUES
 ('88888888-0000-4000-8000-000000000001', '11111111-1111-4111-8111-111111111111', 'animal', '2026-10-15 10:00:00', '2026-10-15 10:00:00');
INSERT INTO user_point_history (user_point_history_id, user_id, video_id, amount, watch_time, description, created_at, updated_at) VALUES
 ('77777777-0000-4000-8000-000000000001', '11111111-1111-4111-8111-111111111111', 'aaaaaaaa-0000-4000-8000-000000000001', 10, 200, '시청', '2026-10-16 10:00:00', '2026-10-16 10:00:00'),
 ('77777777-0000-4000-8000-000000000002', '11111111-1111-4111-8111-111111111111', NULL, -5, NULL, '사용', '2026-10-17 10:00:00', '2026-10-17 10:00:00');
