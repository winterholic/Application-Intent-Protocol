
CREATE TABLE `comment` (
  `comment_id` varchar(36) NOT NULL COMMENT '댓글 ID (UUID)',
  `video_id` varchar(36) NOT NULL COMMENT '영상 ID (FK)',
  `user_id` varchar(36) NOT NULL COMMENT '작성자 ID (FK)',
  `content` text NOT NULL COMMENT '댓글 내용',
  `is_modified` tinyint(1) DEFAULT NULL COMMENT '수정 여부 (0:원본, 1:수정됨)',
  `is_deleted` tinyint(1) DEFAULT NULL COMMENT '삭제 여부 (0:활성, 1:삭제됨 - 관리자 확인용)',
  `created_at` timestamp NOT NULL COMMENT '작성일시',
  `updated_at` timestamp NOT NULL COMMENT '수정일시',
  PRIMARY KEY (`comment_id`),
  KEY `user_id` (`user_id`),
  KEY `idx_video_comments` (`video_id`,`is_deleted`,`created_at`),
  CONSTRAINT `comment_ibfk_1` FOREIGN KEY (`video_id`) REFERENCES `video` (`video_id`) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT `comment_ibfk_2` FOREIGN KEY (`user_id`) REFERENCES `user` (`user_id`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `user` (
  `user_id` varchar(36) NOT NULL COMMENT '사용자 고유 ID (UUID)',
  `email` varchar(255) NOT NULL COMMENT '이메일 (로그인 ID)',
  `password` varchar(255) NOT NULL COMMENT '암호화된 비밀번호',
  `name` varchar(50) NOT NULL COMMENT '사용자 이름',
  `role` enum('GENERAL','ADMIN','SUPER_ADMIN') NOT NULL COMMENT '사용자 권한',
  `profile_image_id` int DEFAULT NULL COMMENT '프로필 이미지 ID (클라이언트 리소스 매핑용)',
  `is_tutorial_done` tinyint(1) DEFAULT NULL COMMENT '튜토리얼 완료 여부 (0:미완료, 1:완료)',
  `is_verify_email_done` tinyint(1) DEFAULT NULL COMMENT '이메일 인증 완료 여부 (0:미완료, 1:완료)',
  `is_deleted` tinyint(1) DEFAULT NULL COMMENT '탈퇴 여부 (0:활성, 1:탈퇴)',
  `created_at` timestamp NOT NULL COMMENT '생성일시',
  `updated_at` timestamp NOT NULL COMMENT '수정일시 (자동 갱신)',
  PRIMARY KEY (`user_id`),
  UNIQUE KEY `email` (`email`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `user_emotion_dna` (
  `user_id` varchar(36) NOT NULL,
  `dna_type` varchar(50) NOT NULL,
  `dna_data` json NOT NULL,
  `based_on_videos` int NOT NULL,
  `generated_at` timestamp NOT NULL,
  `expires_at` timestamp NOT NULL,
  PRIMARY KEY (`user_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `user_emotion_summary` (
  `user_id` varchar(36) NOT NULL,
  `neutral_seconds` bigint NOT NULL,
  `happy_seconds` bigint NOT NULL,
  `surprise_seconds` bigint NOT NULL,
  `sad_seconds` bigint NOT NULL,
  `angry_seconds` bigint NOT NULL,
  `last_finalized_at` datetime DEFAULT NULL,
  `last_session_id` varchar(64) DEFAULT NULL,
  `lock_version` bigint NOT NULL,
  `created_at` datetime NOT NULL,
  `updated_at` datetime NOT NULL,
  PRIMARY KEY (`user_id`),
  CONSTRAINT `user_emotion_summary_ibfk_1` FOREIGN KEY (`user_id`) REFERENCES `user` (`user_id`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `user_favorite_genre` (
  `user_favorite_genre_id` varchar(36) NOT NULL COMMENT '선호 장르 ID (UUID)',
  `user_id` varchar(36) NOT NULL COMMENT '사용자 ID (FK)',
  `genre` enum('drama','eating','travel','cook','show','information','game','sports','music','animal','beauty','comedy','horror','exercise','vlog','etc') NOT NULL COMMENT '선호 장르',
  `created_at` timestamp NOT NULL COMMENT '생성일시',
  `updated_at` timestamp NOT NULL COMMENT '수정일시',
  PRIMARY KEY (`user_favorite_genre_id`),
  UNIQUE KEY `uk_user_genre` (`user_id`,`genre`),
  CONSTRAINT `user_favorite_genre_ibfk_1` FOREIGN KEY (`user_id`) REFERENCES `user` (`user_id`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `user_point_history` (
  `user_point_history_id` varchar(36) NOT NULL COMMENT '포인트 이력 ID (UUID)',
  `user_id` varchar(36) NOT NULL COMMENT '사용자 ID (FK)',
  `video_id` varchar(36) DEFAULT NULL COMMENT '관련 영상 ID (FK)',
  `amount` int NOT NULL COMMENT '변동 포인트 (양수:획득, 음수:사용)',
  `watch_time` int DEFAULT NULL COMMENT '인정된 시청 시간(초)',
  `description` varchar(255) DEFAULT NULL COMMENT '내역 설명',
  `created_at` timestamp NOT NULL COMMENT '생성일시',
  `updated_at` timestamp NOT NULL COMMENT '수정일시',
  PRIMARY KEY (`user_point_history_id`),
  KEY `user_id` (`user_id`),
  KEY `video_id` (`video_id`),
  CONSTRAINT `user_point_history_ibfk_1` FOREIGN KEY (`user_id`) REFERENCES `user` (`user_id`) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT `user_point_history_ibfk_2` FOREIGN KEY (`video_id`) REFERENCES `video` (`video_id`) ON DELETE SET NULL ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `video` (
  `video_id` varchar(36) NOT NULL COMMENT '영상 고유 ID (UUID) - MongoDB 연결 키 겸용',
  `youtube_url` varchar(50) NOT NULL COMMENT '유튜브 영상 ID (예: dQw4w9WgXcQ)',
  `title` varchar(255) NOT NULL COMMENT '영상 제목',
  `channel_name` varchar(100) DEFAULT NULL COMMENT '채널명',
  `category` enum('drama','eating','travel','cook','show','information','game','sports','music','animal','beauty','comedy','horror','exercise','vlog','etc') NOT NULL COMMENT '카테고리',
  `duration` int DEFAULT NULL COMMENT '영상 길이(초)',
  `view_count` bigint DEFAULT NULL COMMENT '조회수',
  `is_deleted` tinyint(1) DEFAULT NULL COMMENT '삭제 여부 (0:활성, 1:삭제)',
  `created_at` timestamp NOT NULL COMMENT '등록일시',
  `updated_at` timestamp NOT NULL COMMENT '수정일시',
  PRIMARY KEY (`video_id`),
  UNIQUE KEY `youtube_url` (`youtube_url`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `video_bookmark` (
  `video_bookmark_id` varchar(36) NOT NULL COMMENT '북마크 ID (UUID)',
  `video_id` varchar(36) NOT NULL COMMENT '영상 ID (FK)',
  `user_id` varchar(36) NOT NULL COMMENT '사용자 ID (FK)',
  `created_at` timestamp NOT NULL COMMENT '북마크 등록 시간',
  PRIMARY KEY (`video_bookmark_id`),
  UNIQUE KEY `uk_bookmark_user_video` (`user_id`,`video_id`),
  KEY `video_id` (`video_id`),
  CONSTRAINT `video_bookmark_ibfk_1` FOREIGN KEY (`video_id`) REFERENCES `video` (`video_id`) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT `video_bookmark_ibfk_2` FOREIGN KEY (`user_id`) REFERENCES `user` (`user_id`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `video_like` (
  `video_like_id` varchar(36) NOT NULL COMMENT '좋아요 ID (UUID)',
  `video_id` varchar(36) NOT NULL COMMENT '영상 ID (FK)',
  `user_id` varchar(36) NOT NULL COMMENT '사용자 ID (FK)',
  `created_at` timestamp NOT NULL COMMENT '좋아요 누른 시간',
  PRIMARY KEY (`video_like_id`),
  UNIQUE KEY `uk_like_user_video` (`user_id`,`video_id`),
  KEY `video_id` (`video_id`),
  CONSTRAINT `video_like_ibfk_1` FOREIGN KEY (`video_id`) REFERENCES `video` (`video_id`) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT `video_like_ibfk_2` FOREIGN KEY (`user_id`) REFERENCES `user` (`user_id`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `video_request` (
  `video_request_id` varchar(36) NOT NULL COMMENT '영상 요청 ID (UUID)',
  `user_id` varchar(36) NOT NULL COMMENT '요청한 사용자 ID (FK)',
  `youtube_url` varchar(50) NOT NULL COMMENT '유튜브 영상 ID (예: dQw4w9WgXcQ)',
  `youtube_full_url` varchar(255) NOT NULL COMMENT '유튜브 영상 전체 URL (예: https://www.youtube.com/watch?v=dQw4w9WgXcQ)',
  `status` enum('PENDING','ACCEPTED','REJECTED') NOT NULL COMMENT '처리 상태 (대기, 승인, 거절)',
  `admin_comment` varchar(255) DEFAULT NULL COMMENT '관리자 처리 코멘트 (거절 사유 등)',
  `created_at` timestamp NOT NULL COMMENT '요청 일시',
  `updated_at` timestamp NOT NULL COMMENT '처리 일시',
  PRIMARY KEY (`video_request_id`),
  KEY `user_id` (`user_id`),
  CONSTRAINT `video_request_ibfk_1` FOREIGN KEY (`user_id`) REFERENCES `user` (`user_id`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `video_view_log` (
  `video_view_log_id` varchar(36) NOT NULL COMMENT '시청 기록 ID (UUID)',
  `video_id` varchar(36) NOT NULL COMMENT '영상 ID (FK)',
  `user_id` varchar(36) DEFAULT NULL COMMENT '사용자 ID (FK, 비로그인 시 NULL)',
  `guest_token` varchar(255) DEFAULT NULL COMMENT '비로그인 사용자 식별 토큰 (브라우저/세션 ID)',
  `created_at` timestamp NOT NULL COMMENT '시청 일시',
  `updated_at` timestamp NOT NULL COMMENT '수정일시',
  PRIMARY KEY (`video_view_log_id`),
  KEY `idx_user_history` (`user_id`,`created_at`),
  KEY `idx_video_history` (`video_id`,`created_at`),
  CONSTRAINT `video_view_log_ibfk_1` FOREIGN KEY (`video_id`) REFERENCES `video` (`video_id`) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT `video_view_log_ibfk_2` FOREIGN KEY (`user_id`) REFERENCES `user` (`user_id`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
