#!/usr/bin/env -S sh -c 'exec "$(command -v hangar-python || command -v python3)" "$0" "$@"'
"""Watch a GitHub PR and print one JSON line per event worth waking the agent for.

Run under the Monitor tool: stdout is the event stream. State lives in
~/.cache/hangar-pr-watch/ so re-arming after the 30 min Monitor cap does not re-emit.
"""
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

QUERY = """
query($o:String!,$r:String!,$n:Int!){viewer{login}
 repository(owner:$o,name:$r){viewerPermission pullRequest(number:$n){
  state isDraft mergeable headRefOid url
  reviewThreads(first:100){totalCount nodes{id isResolved isOutdated path line
   root:comments(first:1){nodes{databaseId}}
   comments(last:1){nodes{databaseId author{login} url body}}}}
  reviews(last:30){nodes{databaseId author{login} state commit{oid} body url}}
  comments(last:30){nodes{databaseId author{login} url body}}
  commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){totalCount nodes{__typename
   ... on CheckRun{name status conclusion} ... on StatusContext{context state}}}}}}}}}}
"""
BOT = "coderabbitai"
OK = {"SUCCESS", "SKIPPED", "NEUTRAL"}
PENDING = {"PENDING", "EXPECTED", "QUEUED", "IN_PROGRESS", "WAITING", "REQUESTED"}

# Windows pipes default to the ANSI codepage, which cannot encode emoji in bot comments.
sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")


class Fatal(Exception):
    """Retrying will not help: stop the watch and say why."""


def emit(obj):
    print(json.dumps(obj, ensure_ascii=False), flush=True)


def preview(text, n=240):
    text = re.sub(r"<!--.*?-->", "", text or "", flags=re.S)
    return " ".join(text.split())[:n]


def fetch(owner, repo, number):
    out = subprocess.run(
        ["gh", "api", "graphql", "-f", f"query={QUERY}", "-F", f"o={owner}", "-F", f"r={repo}", "-F", f"n={number}"],
        capture_output=True, text=True, encoding="utf-8", timeout=60,
    )
    data = json.loads(out.stdout or "{}", strict=False)
    if any(e.get("type") == "NOT_FOUND" for e in data.get("errors") or []):
        raise Fatal(f"PR {number} not found in {owner}/{repo}")
    if out.returncode != 0:
        raise RuntimeError(out.stderr.strip()[:300])
    if data.get("errors"):
        raise RuntimeError(str(data["errors"])[:300])
    repository = data["data"].get("repository") or {}
    pr = repository.get("pullRequest")
    if pr is None:
        raise Fatal(f"PR {number} not found in {owner}/{repo}")
    pr["viewerPermission"] = repository.get("viewerPermission")
    return data["data"]["viewer"]["login"], pr


def snapshot(me, pr):
    head = pr["headRefOid"]
    if pr["reviewThreads"]["totalCount"] > 100:
        raise Fatal("more than 100 review threads; the watcher only reads the first page")
    threads, bot_tails = [], {}
    for t in pr["reviewThreads"]["nodes"]:
        last = (t["comments"]["nodes"] or [{}])[-1]
        item = {
            "id": t["id"], "root_comment_id": (t["root"]["nodes"] or [{}])[0].get("databaseId"),
            "path": t["path"], "line": t["line"], "outdated": t["isOutdated"], "resolved": t["isResolved"],
            "last_by": (last.get("author") or {}).get("login"), "last_id": last.get("databaseId"),
            "url": last.get("url"), "text": preview(last.get("body")),
        }
        if item["last_by"] == BOT:
            bot_tails[t["id"]] = item
        if not t["isResolved"]:
            threads.append(item)
    reviews = [
        {"id": r["databaseId"], "url": r["url"], "nitpicks": "Nitpick comments" in (r["body"] or ""),
         "text": preview(r["body"])}
        for r in pr["reviews"]["nodes"]
        if (r["author"] or {}).get("login") == BOT and (r["commit"] or {}).get("oid") == head
    ]
    bot_comments = [
        {"id": c["databaseId"], "url": c["url"], "text": preview(c["body"])}
        for c in pr["comments"]["nodes"] if (c["author"] or {}).get("login") == BOT
    ]
    checks, coderabbit = [], None
    rollup = (pr["commits"]["nodes"] or [{}])[0].get("commit", {}).get("statusCheckRollup") or {}
    contexts = rollup.get("contexts") or {}
    if contexts.get("totalCount", 0) > 100:
        raise Fatal("more than 100 checks; the watcher only reads the first page")
    # A list, not a dict: jobs with the same name must not hide each other's failure.
    for c in contexts.get("nodes", []):
        if c["__typename"] == "StatusContext":
            name, st = c["context"], c["state"]
        else:
            name, st = c["name"], (c["conclusion"] or c["status"])
        if name == "CodeRabbit":
            coderabbit = st
        else:
            checks.append((name, st))
    return {
        "head": head, "state": pr["state"], "draft": pr["isDraft"], "mergeable": pr["mergeable"],
        "coderabbit": coderabbit, "threads": threads, "me": me,
        "viewer_permission": pr.get("viewerPermission"),
        "reviews_on_head": reviews, "bot_comments": bot_comments[-3:],
        "failed": sorted({n for n, s in checks if s not in OK and s not in PENDING}),
        "pending": sorted({n for n, s in checks if s in PENDING}), "url": pr["url"],
        "_bot_tails": bot_tails,
    }


def wake_signature(s):
    """What changes here deserves a wake-up; pending CI progress and the agent's own notes do not."""
    key = {
        "head": s["head"], "coderabbit": s["coderabbit"], "failed": s["failed"],
        "mergeable": s["mergeable"] == "CONFLICTING", "draft": s["draft"],
        "threads": sorted((t["id"], t["last_id"]) for t in s["threads"] if t["last_by"] != s["me"]),
        "reviews": sorted(r["id"] for r in s["reviews_on_head"]),
        "bot_comments": [c["id"] for c in s["bot_comments"]],
    }
    return hashlib.sha1(json.dumps(key, sort_keys=True).encode()).hexdigest()


def ready(s):
    return (s["state"] == "OPEN" and not s["draft"] and s["mergeable"] == "MERGEABLE"
            and s["coderabbit"] == "SUCCESS" and not s["threads"] and not s["failed"] and not s["pending"])


def load_state(path):
    try:
        data = json.loads(path.read_text())
        return data if isinstance(data, dict) else {}
    except FileNotFoundError:
        return {}
    except ValueError:
        emit({"event": "state_reset", "reason": f"unreadable {path}; replaying the current snapshot"})
        return {}


def save_state(path, st):
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(st))
    os.replace(tmp, path)


def watch(a, owner, repo):
    state_file = Path.home() / ".cache" / "hangar-pr-watch" / f"{owner}_{repo}_{a.pr}.json"
    state_file.parent.mkdir(parents=True, exist_ok=True)
    st = {} if a.reset else load_state(state_file)
    st.setdefault("started", time.time())
    save_state(state_file, st)

    window_end = time.time() + a.window
    failures, stable_since, stable_sig, ready_sent = 0, 0.0, None, None
    while True:
        now = time.time()
        if now - st["started"] > a.duration:
            state_file.unlink(missing_ok=True)
            emit({"control": "expired", "pr": a.pr})
            return
        if now > window_end:
            emit({"control": "window_ended", "pr": a.pr})
            return
        try:
            me, pr = fetch(owner, repo, a.pr)
            s = snapshot(me, pr)
        except Fatal:
            raise
        except Exception as e:  # network, auth, rate limit: retry silently, warn once
            failures += 1
            if failures == 3:
                emit({"event": "query_failed", "pr": a.pr, "error": str(e)})
            time.sleep(a.interval)
            continue
        if failures >= 3:
            emit({"event": "query_recovered", "pr": a.pr})
        failures = 0

        if s["state"] != "OPEN":
            state_file.unlink(missing_ok=True)
            emit({"event": "closed", "pr": a.pr, "state": s["state"], "url": s["url"]})
            return

        # A bot reply in a thread already resolved never shows up in `threads`: track its last note.
        tails = {tid: t["last_id"] for tid, t in s["_bot_tails"].items()}
        seen = st.get("bot_tails")
        reopened = [] if seen is None else [
            t for tid, t in s["_bot_tails"].items() if t["resolved"] and seen.get(tid) != t["last_id"]]

        sig = wake_signature(s)
        if sig != st.get("sig") or reopened:
            st["sig"], st["bot_tails"] = sig, tails
            save_state(state_file, st)
            body = {k: v for k, v in s.items() if not k.startswith("_") and k != "me"}
            emit({"event": "change", "pr": a.pr, **body, "replies_in_resolved_threads": reopened})
        elif seen is None:
            st["bot_tails"] = tails
            save_state(state_file, st)

        full = sig + json.dumps([s["pending"], s["mergeable"], len(s["threads"])])
        if ready(s):
            if full != stable_sig:
                stable_sig, stable_since = full, now
            elif now - stable_since >= a.stable and ready_sent != full:
                # Deduped in memory only: a re-arm after a lost turn must announce `ready` again.
                ready_sent = full
                emit({"event": "ready", "pr": a.pr, "head": s["head"], "url": s["url"]})
        else:
            stable_sig = None
        time.sleep(min(a.interval, 30) if ready(s) else a.interval)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pr", type=int)
    ap.add_argument("--repo", help="owner/name; default: the GitHub repo of the current directory")
    ap.add_argument("--interval", type=int, default=60)
    ap.add_argument("--stable", type=int, default=90)
    ap.add_argument("--window", type=int, default=29 * 60)
    ap.add_argument("--duration", type=int, default=24 * 3600)
    ap.add_argument("--reset", action="store_true", help="forget the saved state and emit the current snapshot")
    ap.add_argument("--once", action="store_true", help="print the current snapshot and exit")
    a = ap.parse_args()
    if not a.repo:
        out = subprocess.run(["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"],
                             capture_output=True, text=True, encoding="utf-8", timeout=60)
        if out.returncode != 0:
            raise Fatal(f"no --repo and the current directory has no GitHub repo: {out.stderr.strip()[:200]}")
        a.repo = out.stdout.strip()
    owner, _, repo = a.repo.partition("/")
    if not owner or not repo:
        raise Fatal(f"--repo must be owner/name, got {a.repo!r}")

    if a.once:
        s = snapshot(*fetch(owner, repo, a.pr))
        s["ready"] = ready(s)
        emit({k: v for k, v in s.items() if not k.startswith("_")})
        return
    watch(a, owner, repo)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
    except Exception as e:  # any death must reach the agent as an event, not only stderr
        emit({"control": "crashed", "error": f"{type(e).__name__}: {e}"})
        sys.exit(1)
