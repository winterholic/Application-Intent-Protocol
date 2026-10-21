"""WBS 7.2.5: writes slice13f-youtube_watching_data.json, the watch sessions of the `GET watch/video` slice (deterministic: fixed RNG seed).

Run once from the repository root when the cases change: python3 reference/slice13f_make_seed.py
Both slice13f_oracle.py and crates/aip-cli/tests/mysql_facereview_watch.rs load the written file, so they read the same sessions.
"""
import json
import random
from pathlib import Path

EMOTIONS = ("happy", "neutral", "surprise", "sad", "angry")
HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "slice13f-youtube_watching_data.json"
BASE_VIDEO_2 = "bbbbbbbb-0000-4000-8000-000000000002"


def vid(n):
    return f"eeeeeeee-0000-4000-8000-{n:012d}"


def sessions(video, rng, viewers, keys, noise=True):
    """One session per viewer; every viewer has its own subset of `keys` (centisecond strings) with 2-decimal scores."""
    docs = []
    for number in range(viewers):
        timeline = {}
        for key in keys:
            if rng.random() < 0.7:
                timeline[key] = [round(rng.uniform(0, 100), 2) for _ in EMOTIONS]
        if noise and number == 0:
            timeline["12"] = [1.0, 2.0, 3.0]  # too short: skipped by the original
            timeline["abc"] = [10.0, 20.0, 30.0, 40.0, 50.0]  # not a number: skipped by the original
        docs.append({"user_id": f"99999999-0000-4000-8000-{number:012d}", "video_id": video, "emotion_score_timeline": timeline,
                     "dominant_emotion": "neutral", "emotion_percentages": {e: 20.0 for e in EMOTIONS}, "client_info": {"agent": "seed"},
                     "created_at": "2026-03-25T12:00:00"})
    return docs


def build():
    rng = random.Random(7250)
    docs = []
    docs += sessions(vid(1), rng, 5, [str(n) for n in range(0, 6200, 37)])  # 60 s video: keys past the end too
    docs += sessions(vid(2), rng, 3, ["0", "50", "99", "100", "150", "7000"])  # duration NULL -> 1 s
    docs += sessions(vid(3), rng, 2, ["0", "1", "49", "50", "100", "101"], noise=False)  # duration 0 -> 1 s
    docs += sessions(vid(6), rng, 4, [str(n) for n in range(0, 330, 7)], noise=False)  # 3 s video: 3 cs buckets
    docs += sessions(vid(5), rng, 2, [str(n) for n in range(0, 3000, 53)])
    # a viewer without any timeline value and one with an empty timeline do not matter to the original
    docs.append({"user_id": "99999999-0000-4000-8000-0000000000ff", "video_id": vid(1), "emotion_score_timeline": {}})
    docs.append({"user_id": "99999999-0000-4000-8000-0000000000fe", "video_id": vid(1)})
    # the same keys from several viewers of an existing seeded video, with integer scores
    docs.append({"user_id": "99999999-0000-4000-8000-0000000000fd", "video_id": BASE_VIDEO_2, "emotion_score_timeline": {"0": [10, 20, 30, 40, 50], "500": [1, 2, 3, 4, 5]}})
    return docs


if __name__ == "__main__":
    OUT.write_text(json.dumps(build(), ensure_ascii=False, indent=1) + "\n")
    print(f"wrote {OUT}")
