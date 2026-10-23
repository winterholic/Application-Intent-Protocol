import type { Role, SignupTrendPeriod, VideoRequestStatus } from 'types';
import { adminApproval, adminReads, adminWrites } from './aipClient';

// The names, arguments and results of the pinned admin UI's `src/api/admin.ts` (revision ab0386c), over the AIP SDK instead of `/v2/admin/*`.
// A result keeps the axios shape `{ data, status }` so that no screen changes.

export const getUsers = (params: { keyword?: string; is_deleted?: boolean; page?: number; size?: number }) => adminReads.getUsers(params);

export const deactivateUser = (userId: string) => adminWrites.deactivateUser(userId);

export const changeUserRole = (userId: string, role: Role) => adminWrites.changeUserRole(userId, role);

export const getVideos = (params: { keyword?: string; category?: string; page?: number; size?: number }) => adminReads.getVideos(params);

export const deleteVideo = (videoId: string) => adminWrites.deleteVideo(videoId);

export const getVideoRequests = (params: { status?: VideoRequestStatus; page?: number; size?: number }) => adminReads.getVideoRequests(params);

export const approveVideoRequest = (
  requestId: string,
  data: { youtube_title: string; channel_name: string; duration: number; category: string },
) => adminApproval.approveVideoRequest(requestId, data);

export const rejectVideoRequest = (requestId: string, adminComment: string) => adminWrites.rejectVideoRequest(requestId, adminComment);

export const getComments = (params: { video_id?: string; keyword?: string; is_deleted?: boolean; page?: number; size?: number }) =>
  adminReads.getComments(params);

export const deleteComment = (commentId: string) => adminWrites.deleteComment(commentId);

export const getSystemStatus = () => adminReads.getSystemStatus();

export const getBusinessStats = () => adminReads.getBusinessStats();

export const getSignupTrend = (period: SignupTrendPeriod) => adminReads.getSignupTrend(period);
