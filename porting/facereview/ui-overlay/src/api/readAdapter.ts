import type {
  EmotionSummaryResponse,
  SearchVideoResponse,
  VideoDataType,
  VideoDetailType,
  VideoDistributionDataType,
  VideoRelatedType,
  VideoWatchedType,
} from 'types';

// WBS 7.2.10 slice 1: the read side of the FaceReview player's data access, over the AIP SDK instead of the original REST API.
//
// The functions keep the names, arguments and results of the pinned UI's `src/api/youtube.ts` (revision 2e7c021), so no screen changes.
// Each one calls a declared extension of `facereview.aip` and unwraps what the extension had to flatten: a read extension answers with
// scalars only, so the lists and objects of the original response travel as JSON text (`videos`, `categories`, `timeline_data`, `body`).
//
// Not here on purpose: guests (an extension call needs a signed session, so a guest's home screen is not served: see D-P-UI-ADAPTER),
// bookmark and like toggles, comments, auth, the request form and the Socket.IO player. They are later slices; `youtube.ts` of the
// overlay throws for what is not ported instead of falling back to the original backend.

/** What the adapter needs of the SDK client (`connect()` of `@aip/sdk` has all three). */
export interface AipClient {
  read(query: Record<string, unknown>): Promise<{ ok: true; rows: Record<string, unknown>[] }>;
  extension(name: string, input: Record<string, unknown>): Promise<Record<string, unknown>>;
  writeExtension(
    name: string,
    input: Record<string, unknown>,
    options?: { key?: string },
  ): Promise<{ ok: true; output: Record<string, unknown> } | { ok: false; code: string; msg?: string; retry?: string }>;
  apply(request: Record<string, unknown>, options?: { key?: string }): Promise<{
    ok: boolean;
    code?: string;
    created?: Array<{ resource: string; id: string | number | null }>;
  }>;
}

export interface AdapterOptions {
  /** A fresh idempotency key for one write. */
  newKey?: () => string;
  warn?: (message: string, detail?: unknown) => void;
}

type SafeListEvidence = {
  extension: string;
  adapter_list_length: number;
  contains_fixture_video: boolean;
};

type SafeListEvidenceGlobals = typeof globalThis & {
  __AIP_SAFE_LIST_FIXTURE_ID__?: string;
  __AIP_SAFE_LIST_EVIDENCE__?: (event: SafeListEvidence) => void;
};

function reportSafeListEvidence(extension: string, videos: VideoDataType[]): void {
  const globals = globalThis as SafeListEvidenceGlobals;
  const fixtureId = globals.__AIP_SAFE_LIST_FIXTURE_ID__;
  const report = globals.__AIP_SAFE_LIST_EVIDENCE__;
  if (typeof fixtureId !== 'string' || typeof report !== 'function') return;
  report({
    extension,
    adapter_list_length: videos.length,
    contains_fixture_video: videos.some((video) => video.video_id === fixtureId),
  });
}


const fail = (message: string, code: string, detail?: unknown): never => {
  throw Object.assign(new Error(message), { code, detail });
};

/** The JSON a text field carries. A missing or malformed text is an error, never an empty answer. */
export function parseText<T>(value: unknown, what: string): T {
  if (typeof value !== 'string') return fail(`${what}: expected JSON text`, 'CLIENT.PROTOCOL_ERROR', value);
  try {
    return JSON.parse(value) as T;
  } catch {
    return fail(`${what}: not valid JSON`, 'CLIENT.PROTOCOL_ERROR');
  }
}

type Page<T> = { videos: T[]; total: number; page: number; size: number; has_next: boolean };

function unwrapPage<T>(output: Record<string, unknown>, what: string): Page<T> {
  return { ...(output as Omit<Page<T>, 'videos'>), videos: parseText<T[]>(output.videos, `${what}.videos`) };
}

export function createReadApi(client: AipClient, options: AdapterOptions = {}) {
  const newKey = options.newKey ?? (() => globalThis.crypto.randomUUID());
  const warn = options.warn ?? ((message: string, detail?: unknown) => console.warn(message, detail));

  return {
    /** GET home/category. The original ignores `category_name` (it answers with the member's own category lists), and so does this. */
    async getVideoList(_category?: string): Promise<{ category_name: string; videos: VideoDataType[] }[]> {
      const output = await client.extension('Video.homeCategory', {});
      const categories = parseText<{ category_name: string; videos: VideoDataType[] }[]>(output.categories, 'homeCategory.categories') || [];
      reportSafeListEvidence('Video.homeCategory', categories.flatMap((category) => category.videos));
      return categories;
    },

    /** GET home/search. The UI sends no emotion filter, which the extension reads as an empty comma separated list. */
    async searchVideos(props: { page: number; size: number; keyword_type: string; keyword: string }): Promise<SearchVideoResponse> {
      const output = await client.extension('Video.homeSearch', {
        emotions: '',
        keyword: props.keyword,
        keyword_type: props.keyword_type,
        page: props.page,
        size: props.size,
      });
      return unwrapPage<VideoDataType>(output, 'homeSearch');
    },

    /** GET home/video/all. */
    async getAllVideo(props: { page: number; size: number; emotion: string }): Promise<VideoDataType[]> {
      const output = await client.extension('Video.homeVideoAll', { emotion: props.emotion, page: props.page, size: props.size });
      const videos = unwrapPage<VideoDataType>(output, 'homeVideoAll').videos;
      reportSafeListEvidence('Video.homeVideoAll', videos);
      return videos;
    },

    /** GET home/personalized. */
    async getPersonalRecommendedVideo(): Promise<VideoDataType[]> {
      const output = await client.extension('Video.homePersonalized', {});
      const videos = parseText<VideoDataType[]>(output.videos, 'homePersonalized.videos');
      reportSafeListEvidence('Video.homePersonalized', videos);
      return videos;
    },

    /**
     * GET watch/video. The original adds one to the view count inside the read and answers with the count from before. The extension
     * only reads, so the increment is the transition `Video.view`, applied after the read. A failed increment is reported and does not
     * hide the video: the member still watches it (the original would have failed the whole request).
     */
    async getVideoDetail(props: { video_id: string }): Promise<VideoDetailType> {
      const output = await client.extension('Video.watchDetail', { video_id: props.video_id });
      const detail = {
        ...output,
        timeline_data: parseText<VideoDistributionDataType>(output.timeline_data, 'watchDetail.timeline_data'),
      } as VideoDetailType;
      try {
        const counted = await client.apply({ apply: 'Video.view', target: { ids: [props.video_id] } }, { key: newKey() });
        if (!counted.ok) warn('the view of the video was not counted', counted.code);
      } catch (error) {
        warn('the view of the video was not counted', error);
      }
      return detail;
    },

    /** GET mypage/videos/recent. The original's defaults: emotion `all`, page 1, size 10. */
    async getRecentVideo(props?: { page?: number; size?: number; emotion?: string }): Promise<VideoWatchedType[]> {
      const output = await client.extension('User.myRecentVideos', {
        emotion: props?.emotion ?? 'all',
        page: props?.page ?? 1,
        size: props?.size ?? 10,
      });
      const body = parseText<{ videos?: VideoWatchedType[] }>(output.body, 'myRecentVideos.body');
      return body.videos ?? [];
    },

    /**
     * GET mypage/emotion/summary. This one is a write extension: the original updates the member's running totals inside the GET. Every
     * call is its own write, so every call has its own key (a repeated key would replay the first answer).
     */
    async getEmotionSummary(): Promise<EmotionSummaryResponse> {
      const result = await client.writeExtension('User.myEmotionSummary', {}, { key: newKey() });
      if (!result.ok) return fail(`emotion summary failed: ${result.code}`, result.code, result);
      return parseText<EmotionSummaryResponse>(result.output.body, 'myEmotionSummary.body');
    },

    /** GET watch/recommended. The UI sends only the video; the original's defaults are page 1 and size 10. */
    async getRelatedVideo(props: { video_id: string }): Promise<VideoRelatedType[]> {
      const output = await client.extension('Video.watchRecommended', { video_id: props.video_id, page: 1, size: 10 });
      return unwrapPage<VideoRelatedType>(output, 'watchRecommended').videos ?? [];
    },

    /** GET home/bookmark. */
    async getBookmarkVideos(props: { page: number; size: number; emotion: string }): Promise<VideoDataType[]> {
      const output = await client.extension('Video.homeBookmark', { emotion: props.emotion, page: props.page, size: props.size });
      return unwrapPage<VideoDataType>(output, 'homeBookmark').videos;
    },
  };
}
