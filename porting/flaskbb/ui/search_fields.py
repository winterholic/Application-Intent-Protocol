"""Search query rules for the FlaskBB port renderer.

Upstream ``flaskbb/forum/forms.py`` ``SearchPageForm`` requires a query of 3 to
50 characters and at least one content type, and ``flaskbb/utils/search.py``
whoosheers index several fields per type that ``whooshee_search`` matches
together. AIP read filters match one declared field each, so the renderer
issues one read per field and merges the rows by id. Fields that the port does
not search (User email, Topic first-post content) are listed in PRODUCT-GAPS.md
G-4.
"""

MIN_LENGTH = 3
MAX_LENGTH = 50

# resource -> fields searched; the order is the order of the merged results.
FIELDS = {
    "topic": ("Topic", ("title", "username")),
    "post": ("Post", ("content", "username", "modified_by")),
    "user": ("User", ("username",)),
    "forum": ("Forum", ("title", "description")),
}
TYPES = ("all", *FIELDS)


def validate(query, search_type):
    """Return form errors for a submitted query; empty when it may be searched."""
    errors = []
    if not query.strip():
        errors.append("The search query is required.")
    elif not MIN_LENGTH <= len(query) <= MAX_LENGTH:
        errors.append(f"The search query must be between {MIN_LENGTH} and {MAX_LENGTH} characters long.")
    if search_type not in TYPES:
        errors.append("The content type is not a valid choice.")
    return errors


def merge_by_id(batches):
    """Concatenate row lists, keeping the first occurrence of each id."""
    seen = set()
    merged = []
    for rows in batches:
        for row in rows:
            if row["id"] not in seen:
                seen.add(row["id"])
                merged.append(row)
    return merged
