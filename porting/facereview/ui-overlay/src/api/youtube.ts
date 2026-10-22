// Overlay of `src/api/youtube.ts` of the pinned UI (2e7c021). Same exports, same arguments, same results; the data comes from the
// AIP service through the SDK (see `readAdapter.ts`). Whatever is not ported yet throws: it never falls back to the original backend.
import { aipClient, aipWrites } from './aipClient';
import { createReadApi } from './readAdapter';

const api = createReadApi(aipClient);

export const {
  getVideoList,
  searchVideos,
  getAllVideo,
  getPersonalRecommendedVideo,
  getVideoDetail,
  getRecentVideo,
  getEmotionSummary,
  getRelatedVideo,
  getBookmarkVideos,
} = api;

export type { CategoryType } from 'constants/index';

/** POST home/bookmark toggles on the server in the original. AIP has explicit create and remove, so this needs the member's current
 *  bookmark state in the screen: a later slice (decision D-P-UI-ADAPTER, open problem 2). */
export const toggleBookmark = aipWrites.toggleBookmark;
