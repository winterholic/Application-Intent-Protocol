# Slice13-D: generate the golden input/expected file for the native-driver integration test
# (spikes/spike-v4-worker/tests/slice13d_home_native.rs). `expected` comes from running the ORIGINAL
# rank_personalized from the pinned revision, with the recent-watching assembly of home_service.py 367-376.
#
#   FACEREVIEW_SOURCE_PATH=/path/to/new-facereview python3 product/tests/slice13d_home_golden.py > product/tests/fixtures/facereview-mysql/slice13d_home_golden.json
import json
import os
import random
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import slice13d_home_oracle_test as o  # noqa: E402

rng = random.Random(1018_13)
pool = o.make_pool(rng, 150)            # <= top_n, so exploration draws nothing and the result is deterministic
cat = {v['video_id']: v['category'] for v in pool}
summaries = []
for i in range(20):
    v = rng.choice(pool)
    d = o.dist(rng)
    summaries.append({'user_id': 1, 'video_id': v['video_id'], 'dominant_emotion': max(d, key=d.get),
                      'emotion_percentages': d, 'created_at': 1000 - i})
summaries[3]['dominant_emotion'] = None     # a session without a dominant emotion is skipped by the original
favorites = ['music', 'game', 'horror']
viewed = [v['video_id'] for v in pool[:25]]
bookmarks = [pool[30]['video_id'], pool[31]['video_id']]

recent = [{'category': cat.get(s['video_id']), 'dominant_emotion': s['dominant_emotion'], 'emotion_percentages': s.get('emotion_percentages', {})}
          for s in summaries if s['dominant_emotion'] is not None]
expected = [v['video_id'] for v in o.ORIG.rank_personalized(
    pool=pool, recent_watching=recent, favorite_genres=favorites, viewed_ids=set(viewed), limit=20, top_n=150, random_n=50)]

categories = {}
for c in ('music', 'game', 'drama'):
    categories[c] = [{k: v[k] for k in ('video_id', 'youtube_url', 'title', 'dominant_emotion', 'dominant_emotion_per')}
                     for v in [x for x in pool if x['category'] == c][:5]]
json.dump({'source': {'repo': 'new-facereview', 'sha': o.ORIGINAL_SHA, 'file': o.ORIGINAL_PATH, 'seed': '1018_13'},
           'pool': pool, 'summaries': summaries, 'favorites': favorites, 'viewed': viewed, 'bookmarks': bookmarks,
           'expected_personalized': expected, 'categories': categories}, sys.stdout, ensure_ascii=False, indent=1, sort_keys=True)
print()
