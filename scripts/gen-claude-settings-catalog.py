#!/usr/bin/env python3
"""Regenerate assets/claude_settings_catalog.json, the options of the Claude Code settings page.

    python scripts/gen-claude-settings-catalog.py
    python scripts/gen-claude-settings-catalog.py --reference ref.md --schema schema.json

The keys, topics, descriptions and types come from the official settings reference; the `env`
variable names come from the JSON schema. Only keys usable in `~/.claude/settings.json` are kept.
It stops on a topic or type it does not understand instead of guessing.
"""

import argparse
import json
import re
import sys
import urllib.request
from pathlib import Path

REFERENCE_URL = "https://code.claude.com/docs/en/settings-reference.md"
SCHEMA_URL = "https://www.schemastore.org/claude-code-settings.json"
OUTPUT = Path(__file__).resolve().parent.parent / "assets" / "claude_settings_catalog.json"

USER_SCOPES = {"Any file", "User or managed", "User, local, or managed", "User"}

TOPICS = {
    "Model and responses": "model",
    "Permission settings": "permissions",
    "Sandbox settings": "sandbox",
    "Memory and context": "memory",
    "Interface and terminal": "interface",
    "Git and attribution": "git",
    "Hooks and automation": "hooks",
    "Plugins and skills": "plugins",
    "MCP": "mcp",
    "Agents, sessions, and worktrees": "agents",
    "Remote, desktop, and notifications": "remote",
    "Authentication and providers": "auth",
    "Updates and versioning": "updates",
    "Tools": "tools",
    "Privacy and telemetry": "privacy",
    "Enterprise and managed settings": "enterprise",
}

OVERRIDES = {
    "advisorModel": {"kind": "string", "suggestions": ["fable", "opus", "sonnet"]},
    "attribution": {"shapes": ["object", "boolean"]},
    "attribution.commit": {"kind": "multiline"},
    "attribution.pr": {"kind": "multiline"},
    "env": {"kind": "env", "shapes": ["object"]},
    "forceLoginOrgUUID": {"kind": "json", "shapes": ["string", "array"]},
}

INDEX_ROW = re.compile(
    r"^\| \[`(?P<key>[^`]+)`\]\(#[^)]*\)\s*\|(?P<description>.*)\|"
    r"\s*(?P<topic>[^|]+?)\s*\|\s*(?P<scope>[^|]+?)\s*\|\s*$"
)
LITERAL = re.compile(r'`"([^"`]*)"`')
BULLET_VALUE = re.compile(r'^\* `"([^"`]*)"`')


def fetch(source):
    if not source.startswith("https://"):
        return Path(source).read_text(encoding="utf-8")
    request = urllib.request.Request(source, headers={"User-Agent": "catalog-generator"})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read().decode("utf-8")


def plain(markdown):
    text = re.sub(r"\[((?:[^\[\]]|\[[^\]]*\])*)\]\([^)]*\)", r"\1", markdown)
    text = text.replace("**", "").replace("`", "").replace("\\|", "|")
    return text.strip()


def sections(lines):
    found, current, fenced = {}, None, False
    for line in lines:
        if line.startswith("```"):
            fenced = not fenced
        elif not fenced and re.match(r"^#{1,4} ", line):
            heading = re.match(r"^### `([^`]+)`", line)
            current = found.setdefault(heading.group(1), []) if heading else None
            continue
        if current is not None:
            current.append(line)
    return found


def field(body, name):
    """The `* **Name**: …` bullet of a section and its indented sub-bullets."""
    prefix = f"* **{name}**: "
    for at, line in enumerate(body):
        if line.startswith(prefix):
            rest = []
            for sub in body[at + 1 :]:
                if not sub.startswith("  "):
                    break
                rest.append(sub.strip())
            return line[len(prefix) :], rest
    return "", []


def example(body, key):
    """The value of `key` in the section's first JSON example that sets it."""
    block = None
    for line in body:
        if block is None:
            if re.match(r"^```json \S*settings\S*\.json", line):
                block = []
        elif line.startswith("```"):
            try:
                value = json.loads("\n".join(block))
                for part in key.split("."):
                    value = value[part]
                return value
            except (ValueError, KeyError, TypeError):
                block = None
        else:
            block.append(line)
    return None


def classify(key, type_line, bullets):
    if type_line.startswith("Boolean"):
        return {"kind": "bool"}
    if type_line.startswith("the string "):
        return {"kind": "enum", "values": LITERAL.findall(type_line)}
    if type_line.startswith("number"):
        fraction = "fraction" in type_line or "between" in type_line
        return {"kind": "number", "integer": not fraction}
    if type_line.startswith("array of objects"):
        return {"kind": "json", "shapes": ["array"]}
    if type_line.startswith("array of"):
        return {"kind": "list"}
    if type_line.startswith("object"):
        return {"kind": "json", "shapes": ["object"]}
    if type_line.startswith("string"):
        if not re.match(r'^string, (one of|`")', type_line) or "such as" in type_line:
            return {"kind": "string"}
        values, open_ended = [], False
        for bullet in bullets:
            leading = BULLET_VALUE.match(bullet)
            if leading and "<" not in leading.group(1):
                values.append(leading.group(1))
            else:
                open_ended = True
        values = values or LITERAL.findall(type_line)
        if not values:
            sys.exit(f"{key}: no enum values in {type_line!r}")
        values = list(dict.fromkeys(values))
        if open_ended:
            return {"kind": "string", "suggestions": values}
        return {"kind": "enum", "values": values}
    sys.exit(f"{key}: unknown type {type_line!r}")


def type_hint(type_line, bullets):
    """The type as plain text, completing a "one of:" with the values its sub-bullets name."""
    hint = plain(type_line).rstrip(":")
    if type_line.endswith("one of:"):
        values = [match.group(1) for match in map(BULLET_VALUE.match, bullets) if match]
        hint = f"{hint} {', '.join(values)}"
    return hint


def build(reference, schema):
    lines = reference.split("\n")
    rows = [match.groupdict() for match in map(INDEX_ROW.match, lines) if match]
    rows = [row for row in rows if row["scope"] in USER_SCOPES]
    if not rows:
        sys.exit("no settings index found in the reference")
    unknown = sorted({row["topic"] for row in rows} - TOPICS.keys())
    if unknown:
        sys.exit(f"unknown topics, add them to TOPICS and the locales: {unknown}")

    keys = [row["key"] for row in rows]
    bodies = sections(lines)
    settings = []
    for row in rows:
        key = row["key"]
        body = bodies.get(key, [])
        description = plain(row["description"])
        deprecated = bool(re.match(r"(Deprecated|Removed)\b", description))
        type_line, bullets = field(body, "Type")
        if type_line:
            shape = classify(key, type_line, bullets)
        elif deprecated:
            shape = {"kind": "json"}
        else:
            sys.exit(f"{key}: no type in the reference")
        if sum(other.startswith(f"{key}.") for other in keys) > 1 and key != "env":
            shape = {"kind": "object", "shapes": ["object"]}
        override = OVERRIDES.get(key, {})
        if override.get("kind", shape["kind"]) != shape["kind"]:
            shape = {}
        entry = {"key": key, "topic": TOPICS[row["topic"]], **shape, **override}
        entry["description"] = description
        if type_line:
            entry["type"] = type_hint(type_line, bullets)
        default = plain(field(body, "Default")[0])
        if default:
            entry["default"] = default
        value = example(body, key)
        if value is not None:
            entry["example"] = value
        if deprecated:
            entry["deprecated"] = True
        settings.append(entry)

    order = list(TOPICS.values())
    used = {entry["topic"] for entry in settings}
    variables = schema["properties"]["env"]["properties"]
    return {
        "topics": [topic for topic in order if topic in used],
        "settings": settings,
        "env": [
            {
                "name": name,
                "description": plain(
                    variables[name].get("description", "").replace("UNDOCUMENTED. ", "")
                ),
            }
            for name in sorted(variables)
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--reference", default=REFERENCE_URL, help="URL or file")
    parser.add_argument("--schema", default=SCHEMA_URL, help="URL or file")
    args = parser.parse_args()

    catalog = build(fetch(args.reference), json.loads(fetch(args.schema)))
    text = json.dumps(catalog, indent=1, ensure_ascii=False) + "\n"
    OUTPUT.write_text(text, encoding="utf-8", newline="\n")
    print(f"{OUTPUT}: {len(catalog['settings'])} settings, {len(catalog['env'])} variables")


if __name__ == "__main__":
    main()
