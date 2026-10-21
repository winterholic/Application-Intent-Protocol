CREATE TABLE `user` (
  `user_id` varchar(36) NOT NULL,
  `name` varchar(50) NOT NULL,
  PRIMARY KEY (`user_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE TABLE `video` (
  `video_id` varchar(36) NOT NULL,
  `title` varchar(100) NOT NULL,
  `view_count` bigint NOT NULL DEFAULT 0,
  PRIMARY KEY (`video_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
