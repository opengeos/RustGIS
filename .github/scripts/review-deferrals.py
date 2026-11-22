"""Warns when a pull request leaves a defect for the release quality pass.

A review point may wait for the release quality pass when the reply to it ends with
`<!-- quality-sweep -->`, but only a quality point may (AGENTS.md, "Review comments"). The Claude
review (claude-code-review.yml, posting as github-actions) ends each inline finding with its
category, `<!-- review-category: bug -->` and so on, so the reviewer classifies a finding and the
author doesn't. This check warns about a deferral reply in a thread that the reviewer opened with
a `bug`, `security` or `agents-md` finding, or whose opening comment someone else has since edited.
It never fails: whoever merges reads the warnings and decides.

Threads opened by anyone else (CodeRabbit, people) or by a review from before categories carry no
category, so they are only listed in a notice: a reviewer there can answer the reply.

CI runs this on each push and again in the merge queue, which sees replies posted after the last
push. It needs GH_TOKEN (read access to pull requests), GITHUB_REPOSITORY, and PR_NUMBER or, in the
merge queue, MERGE_GROUP_REF.
"""

import json
import os
import re
import subprocess
import sys

DEFER = "<!-- quality-sweep -->"
CATEGORY = re.compile(r"<!--\s*review-category:\s*([a-z-]+)\s*-->")
MUST_FIX = {"bug", "security", "agents-md"}
REVIEWER = "github-actions"
QUEUE_REF = re.compile(r"/pr-(\d+)-[0-9a-f]{40}$")

THREADS = """
query($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 50, after: $endCursor) {
        pageInfo { hasNextPage endCursor }
        nodes { id path line originalLine comments(first: 100) { %s } }
      }
    }
  }
}
"""
COMMENTS = """pageInfo { hasNextPage endCursor }
  nodes { url body lastEditedAt author { login __typename } editor { login } }"""
MORE_COMMENTS = """
query($id: ID!, $endCursor: String) {
  node(id: $id) { ... on PullRequestReviewThread { comments(first: 100, after: $endCursor) { %s } } }
}
"""


def is_reviewer(comment):
    """Whether a comment was posted by the Claude review workflow's bot account."""
    author = comment.get("author") or {}
    return author.get("login") == REVIEWER and author.get("__typename") == "Bot"


def category(opening):
    """The category a reviewer finding gives itself: the most serious of its markers, or None."""
    if not is_reviewer(opening):
        return None
    found = CATEGORY.findall(opening.get("body") or "")
    serious = [c for c in found if c in MUST_FIX]
    return (serious or found or [None])[0]


def edited_by_someone_else(opening):
    """Whether a human or other account edited the reviewer's comment, which may have changed its category."""
    if not opening.get("lastEditedAt"):
        return False
    return ((opening.get("editor") or {}).get("login")) != REVIEWER


def check(threads):
    """Sorts the threads that hold a deferral reply into (flagged, unclassified) lists of messages."""
    flagged, unclassified = [], []
    for thread in threads:
        comments = thread.get("comments") or []
        if not comments:
            continue
        opening, replies = comments[0], comments[1:]
        deferral = next((r for r in replies if DEFER in (r.get("body") or "")), None)
        if deferral is None:
            continue
        where = f"{thread.get('path')}:{thread.get('line') or thread.get('originalLine') or '?'}"
        if not is_reviewer(opening):
            unclassified.append(f"{where} ({(opening.get('author') or {}).get('login', 'ghost')}): {deferral.get('url')}")
            continue
        kind = category(opening)
        if edited_by_someone_else(opening):
            reason = "its finding was edited after the review posted it"
        elif kind in MUST_FIX:
            reason = f"the reviewer classed it as {kind}"
        else:
            if kind is None:
                unclassified.append(f"{where} (review without a category): {deferral.get('url')}")
            continue
        flagged.append(f"{where}: left for the release quality pass, but {reason}. {deferral.get('url')}")
    return flagged, unclassified


def graphql(query, **variables):
    """Runs one GraphQL query through the gh CLI and returns its data."""
    args = ["gh", "api", "graphql", "-f", f"query={query}"]
    for key, value in variables.items():
        args += ["-F" if isinstance(value, int) else "-f", f"{key}={value}"]
    out = subprocess.run(args, check=True, capture_output=True, text=True).stdout
    return json.loads(out)["data"]


def fetch(repo, number):
    """The pull request's review threads, every comment of each in order."""
    owner, name = repo.split("/", 1)
    threads, cursor = [], None
    while True:
        extra = {"endCursor": cursor} if cursor else {}
        pr = graphql(THREADS % COMMENTS, owner=owner, name=name, number=number, **extra)["repository"]["pullRequest"]
        page = pr["reviewThreads"]
        for node in page["nodes"]:
            comments, more = node["comments"]["nodes"], node["comments"]["pageInfo"]
            while more["hasNextPage"]:
                rest = graphql(MORE_COMMENTS % COMMENTS, id=node["id"], endCursor=more["endCursor"])["node"]["comments"]
                comments, more = comments + rest["nodes"], rest["pageInfo"]
            threads.append({**node, "comments": comments})
        if not page["pageInfo"]["hasNextPage"]:
            return threads
        cursor = page["pageInfo"]["endCursor"]


def pr_number():
    """The pull request to check, from PR_NUMBER or the merge queue's branch name."""
    if os.environ.get("PR_NUMBER"):
        return int(os.environ["PR_NUMBER"])
    match = QUEUE_REF.search(os.environ.get("MERGE_GROUP_REF", ""))
    if match is None:
        raise SystemExit("::error::No PR_NUMBER, and MERGE_GROUP_REF names no pull request.")
    return int(match.group(1))


def escape(text):
    """Text for a workflow command line: a path with a newline must not start a command of its own."""
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def main():
    """Lists the pull request's deferrals of findings that must be fixed, as warnings. Never fails for them."""
    number = pr_number()
    flagged, unclassified = check(fetch(os.environ["GITHUB_REPOSITORY"], number))
    for line in unclassified:
        print(f"::notice::Deferred without a review category, not checked: {escape(line)}")
    for line in flagged:
        print(f"::warning::{escape(line)}")
    if flagged:
        report = (
            f"#{number} leaves {len(flagged)} finding(s) for the release quality pass that the review classed as "
            "must-fix. Fix each in the pull request, or link a filed issue in the reply for a defect too large "
            "for it. If the review misclassified one, say so in its thread."
        )
    else:
        report = f"#{number}: no deferral to the release quality pass is on a finding that must be fixed."
    print(report)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
            summary.write("\n".join([report, "", *(f"- {line}" for line in flagged)]) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
