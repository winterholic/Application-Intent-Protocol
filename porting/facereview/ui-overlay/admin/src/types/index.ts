export type Role = 'GENERAL' | 'ADMIN' | 'SUPER_ADMIN';

export interface AdminUser {
  user_id: string;
  email: string;
  name: string;
  role: Role;
  profile_image_id: number;
  is_tutorial_done: boolean;
  is_verify_email_done: boolean;
  is_deleted: boolean;
  created_at: string;
  total_watch_count: number;
  total_comment_count: number;
}

export interface GetUsersResponse {
  users: AdminUser[];
  total: number;
  page: number;
  size: number;
  has_next: boolean;
}

export type VideoRequestStatus = 'PENDING' | 'ACCEPTED' | 'REJECTED';

export interface AdminVideoRequest {
  video_request_id: string;
  user_id: string;
  user_name: string;
  youtube_url: string;
  youtube_full_url: string;
  status: VideoRequestStatus;
  admin_comment: string;
  created_at: string;
  updated_at: string;
}

export interface GetVideoRequestsResponse {
  requests: AdminVideoRequest[];
  total: number;
  page: number;
  size: number;
  has_next: boolean;
}

export interface AdminVideo {
  video_id: string;
  youtube_url: string;
  title: string;
  channel_name: string;
  category: string;
  duration: number;
  view_count: number;
  like_count: number;
  comment_count: number;
  created_at: string;
  is_deleted: boolean;
}

export interface GetVideosResponse {
  videos: AdminVideo[];
  total: number;
  page: number;
  size: number;
  has_next: boolean;
}

export interface AdminComment {
  comment_id: string;
  video_id: string;
  video_title: string;
  user_id: string;
  user_name: string;
  content: string;
  is_modified: boolean;
  is_deleted: boolean;
  created_at: string;
}

export interface GetCommentsResponse {
  comments: AdminComment[];
  total: number;
  page: number;
  size: number;
  has_next: boolean;
}

export interface SystemStatusResponse {
  server: {
    cpu_usage: number | null;
    memory_usage: number | null;
    memory_total_mb: number | null;
    disk_usage: number | null;
  };
  api: {
    total_requests_1h: number;
    avg_response_time_ms: number;
    error_rate_1h: number;
  };
  connections: {
    mysql: 'ok' | 'error';
    redis: 'ok' | 'error';
    mongodb: 'ok' | 'error';
  };
  checked_at: string;
}

export interface SignupTrendPoint {
  date: string;
  count: number;
}

export type SignupTrendPeriod = '7d' | '30d' | '3m' | '1y' | '3y';

export interface SignupTrendResponse {
  points: SignupTrendPoint[];
  granularity: 'day' | 'week' | 'month';
}

export interface VideoRequestPipeline {
  pending_count: number;
  avg_processing_minutes: number | null;
}

export interface CategoryPopularity {
  category: string;
  view_count: number;
}

export interface EmotionDistribution {
  neutral: number;
  happy: number;
  surprise: number;
  sad: number;
  angry: number;
}

export interface DominantEmotionCount {
  emotion: string;
  video_count: number;
}

export interface ContentHealth {
  avg_completion_rate: number;
  emotion_distribution: EmotionDistribution;
  category_top5: CategoryPopularity[];
  dominant_emotion_video_counts: DominantEmotionCount[];
}

export interface BusinessStatsResponse {
  signup_trend: SignupTrendPoint[];
  weekly_active_users: number;
  video_request_pipeline: VideoRequestPipeline;
  content_health: ContentHealth;
  computed_at: string;
}

export interface LoginResponse {
  access_token: string;
}

export interface MeResponse {
  user_id: string;
  email: string;
  name: string;
  profile_image_id: number;
  is_tutorial_done: boolean;
  is_verify_email_done: boolean;
  role: Role;
  created_at: string;
  favorite_genres: string[];
}

export interface MessageResponse {
  message: string;
}
