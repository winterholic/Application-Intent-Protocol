import json
import unittest
from pathlib import Path

FIXTURE = json.loads(Path(__file__).with_name("topic-move-regression.json").read_text())
USERNAMES = {2: "member-alice", 3: "admin-bob"}
FIELDS = ("last_post", "last_post_user", "last_post_title", "last_post_username", "last_post_created")


def candidate_projection(post, topics):
    topic = topics[post["topic"]]
    return dict(zip(FIELDS, (post["source_id"], post["user"], topic["title"],
                             USERNAMES[post["user"]], post["date_created"])))


def source_forum_after_move():
    """Candidates of Forum 1 once the moving Topic has left, per FlaskBB update_last_post."""
    topics = {item["key"]: item for item in FIXTURE["topics"]}
    moving = topics["moving"]
    return topics, [post for post in FIXTURE["posts"]
                    if topics[post["topic"]]["forum"] == 1 and post["topic"] != "moving"], moving


def apply_move(unconditional):
    topics, candidates, _ = source_forum_after_move()
    current = FIXTURE["before_move_projection"]["source"]
    if not candidates:
        return dict.fromkeys(FIELDS)
    latest = max(candidates, key=lambda post: post["date_created"])
    # Source update_last_post writes only when `last_post != self.last_post`.
    if latest["source_id"] == current["last_post"] and not unconditional:
        return dict(current)
    return candidate_projection(latest, topics)


class TopicMoveStaleProjectionControl(unittest.TestCase):
    def test_premise_current_pointer_is_the_only_maximal_candidate(self):
        _, candidates, _ = source_forum_after_move()
        latest = max(post["date_created"] for post in candidates)
        winners = [post["source_id"] for post in candidates if post["date_created"] == latest]
        self.assertEqual(winners, [FIXTURE["before_move_projection"]["source"]["last_post"]])

    def test_pointer_preservation_matches_shared_expectation(self):
        self.assertEqual(apply_move(unconditional=False),
                         FIXTURE["after_move"]["forum_projection"]["source"])

    def test_unconditional_update_negative_control_is_detected(self):
        rewritten = apply_move(unconditional=True)
        self.assertNotEqual(rewritten, FIXTURE["after_move"]["forum_projection"]["source"])
        for field in FIELDS[1:]:
            self.assertNotEqual(rewritten[field], FIXTURE["before_move_projection"]["source"][field], field)
        print("NEGATIVE_CONTROL unconditional_topic_move_update_rejected rewritten={}".format(
            json.dumps(rewritten, sort_keys=True)))


if __name__ == "__main__":
    unittest.main(verbosity=2)
