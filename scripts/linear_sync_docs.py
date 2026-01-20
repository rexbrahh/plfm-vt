#!/usr/bin/env python3
import argparse
import os
import re
import sys
from pathlib import Path
from typing import Dict, List, Optional, Tuple

import json
import urllib.error
import urllib.request


LINEAR_API_URL = "https://api.linear.app/graphql"

PROJECT_IDS = {
    "Docs: ADRs (Locked)": "e1fc9ac8-c77a-4166-8108-bda48af1439a",
    "Docs: Architecture": "b4270666-78bf-4470-a0f0-ec9e9a249396",
    "Docs: CLI": "5c99c1c2-bb8f-46f8-99c0-50f9235e0256",
    "Docs: Engineering": "951b0719-0595-4a32-b686-69ccd4416662",
    "Docs: Frontend": "7705aea0-e18f-4d2f-a541-7972fbde5951",
    "Docs: Ops": "d6b4c854-792b-42d4-a6df-3259b8c2368b",
    "Docs: Product": "f641eadc-5e6e-40f3-a400-466974819fb8",
    "Docs: Runtime": "51e9b52b-4842-47ce-9ab3-8ebf6609cb82",
    "Docs: Specs": "6f247d46-c8f4-4558-b57d-beb9144bacdc",
    "Docs: Security": "6ae72cf9-6098-478c-83c7-ea6ea990b64a",
    "Docs: Archive": "7e5d6cdc-116d-411d-82e9-706a55b5f35f",
    "Docs: Navigation & Glossary": "1b7293b6-d74d-4048-944f-cd3f37e01cd0",
}


ISSUES_QUERY = """
query Issues($projectId: ID!, $after: String) {
  issues(filter: {project: {id: {eq: $projectId}}}, first: 100, after: $after) {
    nodes { id title description }
    pageInfo { hasNextPage endCursor }
  }
}
"""


ISSUE_UPDATE_MUTATION = """
mutation IssueUpdate($id: String!, $description: String!) {
  issueUpdate(id: $id, input: { description: $description }) {
    success
    issue { id }
  }
}
"""


MIRROR_META_RE = re.compile(r"```mirror-meta\s+([\s\S]*?)```", re.MULTILINE)
SOURCE_PATH_RE = re.compile(r"^\s*source_path:\s*(.+?)\s*$", re.MULTILINE)


def graphql_request(api_key: str, query: str, variables: dict) -> dict:
    payload = json.dumps({"query": query, "variables": variables}).encode("utf-8")
    req = urllib.request.Request(
        LINEAR_API_URL,
        data=payload,
        headers={
            "Content-Type": "application/json",
            "Authorization": api_key,
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8") if exc.fp else ""
        raise RuntimeError(f"Linear API HTTP {exc.code}: {body}") from exc
    data = json.loads(raw)
    if "errors" in data:
        raise RuntimeError(f"Linear API error: {data['errors']}")
    return data["data"]


def fetch_issues(api_key: str, project_id: str) -> List[dict]:
    issues = []
    after = None
    while True:
        data = graphql_request(api_key, ISSUES_QUERY, {"projectId": project_id, "after": after})
        page = data["issues"]
        issues.extend(page["nodes"])
        if not page["pageInfo"]["hasNextPage"]:
            break
        after = page["pageInfo"]["endCursor"]
    return issues


def extract_meta_block(description: str) -> Optional[str]:
    match = MIRROR_META_RE.search(description or "")
    if not match:
        return None
    return f"```mirror-meta\n{match.group(1)}```"


def extract_source_path(meta_block: str) -> Optional[str]:
    match = SOURCE_PATH_RE.search(meta_block or "")
    if not match:
        return None
    return match.group(1).strip()


def transform_markdown(text: str) -> str:
    lines = text.split("\n")
    out = []
    in_code = False
    for line in lines:
        stripped = line.lstrip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            in_code = not in_code
            out.append(line)
            continue
        if in_code:
            out.append(line)
            continue

        if stripped.startswith("- [ ]") or stripped.startswith("- [x]") or stripped.startswith("- [X]"):
            out.append(line)
            continue

        blockquote_match = re.match(r"^(\s*(?:>\s*)*)(.*)$", line)
        if blockquote_match:
            prefix = blockquote_match.group(1)
            rest = blockquote_match.group(2)
        else:
            prefix = ""
            rest = line

        converted = False
        for marker in ("- ", "* ", "+ "):
            if rest.startswith(marker):
                content = rest[len(marker):]
                out.append(f"{prefix}- [ ] {content}")
                converted = True
                break
        if converted:
            continue

        ordered_match = re.match(r"^(\d+)\.\s+(.*)$", rest)
        if ordered_match:
            content = ordered_match.group(2)
            out.append(f"{prefix}- [ ] {content}")
            continue

        out.append(line)

    return "\n".join(out)


def build_mapping(api_key: str) -> Dict[str, Tuple[str, str]]:
    mapping = {}
    for project_name, project_id in PROJECT_IDS.items():
        issues = fetch_issues(api_key, project_id)
        for issue in issues:
            meta_block = extract_meta_block(issue.get("description") or "")
            if not meta_block:
                continue
            source_path = extract_source_path(meta_block)
            if not source_path:
                continue
            mapping[source_path] = (issue["id"], meta_block)
    return mapping


def build_description(meta_block: str, content: str) -> str:
    if not meta_block:
        return content
    return f"{meta_block}\n\n{content}"


def sync_docs(api_key: str, apply: bool, limit: Optional[int]) -> int:
    mapping = build_mapping(api_key)
    repo_docs = sorted([str(p) for p in Path("docs").rglob("*.md")])

    missing = [path for path in repo_docs if path not in mapping]
    if missing:
        print("Missing Linear issues for paths:")
        for path in missing:
            print(f"- {path}")
        print("Aborting due to missing mappings.")
        return 1

    if limit:
        repo_docs = repo_docs[:limit]

    updates = []
    for path in repo_docs:
        issue_id, meta_block = mapping[path]
        text = Path(path).read_text()
        transformed = transform_markdown(text)
        description = build_description(meta_block, transformed)
        updates.append((issue_id, description, path))

    print(f"Prepared {len(updates)} updates.")
    if not apply:
        for issue_id, _, path in updates[:5]:
            print(f"- {path} -> {issue_id}")
        print("Dry run only. Use --apply to update issues.")
        return 0

    for issue_id, description, path in updates:
        graphql_request(api_key, ISSUE_UPDATE_MUTATION, {"id": issue_id, "description": description})
        print(f"Updated {path}")

    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Sync docs to Linear issues with checkbox conversion.")
    parser.add_argument("--apply", action="store_true", help="Apply updates to Linear")
    parser.add_argument("--limit", type=int, default=None, help="Limit number of docs to process")
    args = parser.parse_args()

    api_key = os.environ.get("LINEAR_API_KEY")
    if not api_key:
        print("LINEAR_API_KEY is required in environment.")
        return 1

    return sync_docs(api_key, apply=args.apply, limit=args.limit)


if __name__ == "__main__":
    sys.exit(main())
