#!/usr/bin/env python3
"""Generate the offline Docker fallback from pinned official YAML references.

Only --refresh accesses the network; it fetches the pins below, never a moving
branch. Normal generation and --check read a verified external source cache.
PyYAML is a maintainer dependency only. No Docker executable is used.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import urllib.request

try:
    import yaml
except ImportError:
    sys.exit("PyYAML is required for this maintainer script; install PyYAML==6.0.3 in a venv.")


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT = ROOT / "crates/kiln/src/app/terminal_completion/docker.json"
DEFAULT_CACHE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "kiln/completion-docker"
EXCLUDED = ("hidden", "deprecated", "experimental", "experimentalcli")


@dataclass(frozen=True)
class Source:
    key: str
    repo: str
    commit: str
    # Nonrecursive directories, with expected YAML counts.
    directories: tuple[tuple[str, int], ...]
    extra_files: tuple[str, ...]
    digest: str

    def directory(self, cache: Path) -> Path:
        return cache / self.key / self.commit


# SHA-256 over sorted path + NUL + hex SHA-256(file bytes) + LF, including the
# attribution/provenance files. Updating a pin requires reviewing these counts,
# digests, EXPECTED_COUNTS, and docs/maintainers/completion-sources.md together.
SOURCES = (
    Source(
        "docs", "docker/docs", "1d503f8b7c2b2008da6cbc08bb5be163666c84bf",
        (("data/cli/engine", 190), ("data/cli/buildx", 40)),
        ("LICENSE", "README.md", "hack/sync-cli-docs.sh"),
        "40bfac882381cdec0c9523e2f83b3400d9177cda14c3b9da202090970737d525",
    ),
    Source(
        "compose", "docker/compose", "e18ab3d8797716b565e47b32f0ddb60e052fbbb6",
        (("docs/reference", 47),), ("LICENSE", "NOTICE"),
        "349107b812ded500f56dd4f31d9f02671f3bc6d5d508305d0ec1cd477a291a0e",
    ),
)

# Filled from, and checked against, the reviewed source snapshot. Commands
# include the docker root and documented aliases; options count definitions,
# rather than individual long/short spellings or inherited runtime expansion.
EXPECTED_COUNTS = {
    "commands": 279,
    "options": 1674,
    "option_spellings": 2096,
    "persistent_options": 3,
    "directory_options": 5,
    "file_options": 67,
    "directory_positionals": 5,
}

POPULAR = {
    "docker": ("compose", "run", "ps", "images", "build", "buildx", "logs", "exec", "pull", "push", "stop", "start", "restart", "rm", "rmi"),
    "docker compose": ("up", "down", "ps", "logs", "exec", "run", "build", "pull", "push", "restart", "stop", "start", "config"),
}

# Explicit local-path annotations. Names are looked up in the actual YAML;
# these sets cannot introduce a command or option. Container paths, mount
# expressions, endpoints, resource IDs, and build output expressions stay free
# strings. In particular, --config is a directory only on the Docker root.
FILE_OPTIONS = {
    "file", "filename", "env-file", "env-from-file", "label-file", "iidfile",
    "cidfile", "compose-file", "input", "tlscacert", "tlscert", "tlskey",
    "ca-cert", "ca-key", "metadata-file", "buildkitd-config",
}
DIRECTORY_OPTIONS = {
    ("docker", "config"),
    ("docker compose", "project-directory"),
    ("docker compose bridge convert", "output"),
    ("docker compose bridge convert", "templates"),
    ("docker trust key generate", "dir"),
}
FILE_OUTPUT_COMMANDS = {
    "docker container export", "docker export", "docker image save", "docker save",
    "docker compose config", "docker compose convert", "docker compose export",
    "docker buildx history export",
}
BUILD_CONTEXTS = {"docker build", "docker builder build", "docker image build", "docker buildx build", "docker buildx b"}


def source_digest(files: dict[str, bytes]) -> str:
    digest = hashlib.sha256()
    for path, data in sorted(files.items()):
        digest.update(path.encode("utf-8") + b"\0" + hashlib.sha256(data).hexdigest().encode("ascii") + b"\n")
    return digest.hexdigest()


def selected_path(source: Source, path: str) -> bool:
    p = Path(path)
    return path in source.extra_files or (p.suffix == ".yaml" and p.parent.as_posix() in dict(source.directories))


def verify_source(source: Source, files: dict[str, bytes]) -> None:
    for directory, count in source.directories:
        actual = sum(Path(p).parent.as_posix() == directory and p.endswith(".yaml") for p in files)
        if actual != count:
            raise ValueError(f"{source.repo}@{source.commit}: {directory}: expected {count} YAML files, got {actual}")
    expected = sum(count for _, count in source.directories) + len(source.extra_files)
    if len(files) != expected or not set(source.extra_files).issubset(files):
        raise ValueError(f"{source.repo}: source files/attribution are incomplete; use --refresh")
    actual = source_digest(files)
    if actual != source.digest:
        raise ValueError(f"{source.repo}@{source.commit}: source SHA-256 mismatch: {actual}; use --refresh")


def fetch_bytes(url: str) -> bytes:
    # Public reference sources only. No tokens, Docker config, auth files,
    # subprocesses, daemon APIs, or installed CLI inspection.
    request = urllib.request.Request(url, headers={"User-Agent": "kiln-completion-docs-updater"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read()


def refresh_source(source: Source, cache: Path) -> None:
    tree = json.loads(fetch_bytes(f"https://api.github.com/repos/{source.repo}/git/trees/{source.commit}?recursive=1"))
    if tree.get("truncated") or not isinstance(tree.get("tree"), list):
        raise ValueError(f"{source.repo}: incomplete upstream file tree")
    entries = [entry for entry in tree["tree"] if entry["type"] == "blob" and selected_path(source, entry["path"])]

    def download(entry: dict) -> tuple[str, bytes]:
        path = entry["path"]
        data = fetch_bytes(f"https://raw.githubusercontent.com/{source.repo}/{source.commit}/{path}")
        blob = hashlib.sha1(b"blob " + str(len(data)).encode("ascii") + b"\0" + data).hexdigest()
        if blob != entry["sha"]:
            raise ValueError(f"{source.repo}/{path}: Git blob SHA mismatch")
        return path, data

    with ThreadPoolExecutor(max_workers=8) as pool:
        files = dict(pool.map(download, entries))
    verify_source(source, files)
    directory = source.directory(cache)
    for path, data in sorted(files.items()):
        destination = directory / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)


def read_source(source: Source, cache: Path) -> dict[str, bytes]:
    directory = source.directory(cache)
    paths = [path for prefix, _ in source.directories for path in (directory / prefix).glob("*.yaml")]
    paths.extend(directory / path for path in source.extra_files)
    try:
        files = {p.relative_to(directory).as_posix(): p.read_bytes() for p in paths}
    except FileNotFoundError as error:
        raise ValueError(f"Missing source cache for {source.repo}; run once with --refresh --cache-dir {cache}") from error
    verify_source(source, files)
    return files


def visible(record: dict) -> bool:
    for key in EXCLUDED:
        if key in record and not isinstance(record[key], bool):
            raise ValueError(f"Invalid {key} flag in {record.get('command', record.get('option'))}")
    return not any(record.get(key, False) for key in EXCLUDED)


def text(value: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"Expected documentation text, got {value!r}")
    return " ".join(value.split())


def ancestors(command: str):
    words = command.split()
    for length in range(len(words) - 1, 0, -1):
        yield " ".join(words[:length])


def parse_documents(snapshots: dict[str, dict[str, bytes]]) -> dict[str, dict]:
    documents = {}
    for source in SOURCES:
        for path, data in sorted(snapshots[source.key].items()):
            if not path.endswith(".yaml"):
                continue
            document = yaml.safe_load(data)
            command = document.get("command", "")
            if not re.fullmatch(r"docker(?: [a-zA-Z0-9][a-zA-Z0-9-]*)*", command):
                raise ValueError(f"{source.repo}/{path}: invalid command: {command!r}")
            if command in documents:
                raise ValueError(f"Duplicate upstream command: {command}")
            documents[command] = document
    return documents


def aliases(document: dict) -> list[str]:
    value = document.get("aliases", "")
    if not isinstance(value, str):
        raise ValueError(f"Invalid alias list for {document['command']}")
    result = [document["command"]]
    for alias in value.split(","):
        alias = alias.strip()
        if alias and alias not in result:
            if not re.fullmatch(r"docker(?: [a-zA-Z0-9][a-zA-Z0-9-]*)+", alias):
                raise ValueError(f"Invalid documented alias: {alias!r}")
            result.append(alias)
    return result


def option_provider(command: str, option: str) -> str:
    if (command, option) in DIRECTORY_OPTIONS:
        return "directories"
    if option in FILE_OPTIONS or (option == "output" and command in FILE_OUTPUT_COMMANDS):
        return "files"
    if command == "docker trust signer add" and option == "key":
        return "files"
    return ""


def make_option(command: str, raw: dict, persistent: bool) -> dict:
    option = raw["option"]
    if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9-]*", option):
        raise ValueError(f"Invalid option for {command}: {option!r}")
    names = ["--" + option]
    shorthand = raw.get("shorthand", "")
    if shorthand:
        if not isinstance(shorthand, str) or len(shorthand) != 1:
            raise ValueError(f"Invalid shorthand for {command}: {shorthand!r}")
        names.append("-" + shorthand)
    is_bool = raw["value_type"] == "bool"
    provider = "" if is_bool else option_provider(command, option)
    hint = "" if is_bool else {"directories": "<directory>", "files": "<file>"}.get(provider, "<string>")
    result = {"name": names, "description": text(raw["description"]), "hint": hint, "persistent": persistent}
    if provider:
        result["provider"] = provider
    return result


def generate(documents: dict[str, dict]) -> dict:
    # Drop entire hidden/experimental/deprecated branches. Include every other
    # documented command, even when an upstream parent's cname list omits it.
    stable = {
        name: doc for name, doc in documents.items()
        if visible(doc) and all(parent not in documents or visible(documents[parent]) for parent in ancestors(name))
    }
    groups: dict[str, set[str]] = {}
    for name, doc in sorted(stable.items()):
        names = set(aliases(doc))
        linked = set().union(*(groups.get(alias, set()) for alias in names))
        names.update(linked)
        for alias in names:
            groups[alias] = names

    # Inheritance comes from inherited_options, never a blanket "root=true".
    persistent: set[tuple[str, str]] = set()
    for name, doc in stable.items():
        for raw in doc.get("inherited_options", []):
            if not visible(raw):
                continue
            for parent in ancestors(name):
                owner = next((opt for opt in stable.get(parent, {}).get("options", [])
                              if visible(opt) and opt["option"] == raw["option"] and opt["value_type"] == raw["value_type"]), None)
                if owner:
                    persistent.add((parent, raw["option"]))
                    break

    specs = {}
    for name, equivalents in sorted(groups.items()):
        if any(parent in documents and parent not in stable for parent in [name, *ancestors(name)]):
            continue
        members = [stable[alias] for alias in sorted(equivalents) if alias in stable]
        # Canonical definitions win conflicts. Other documented aliases add
        # missing options, including modern Buildx flags on docker build and
        # bake while preserving separately documented legacy builder flags.
        members.sort(key=lambda doc: (doc["command"] != name, not doc["command"].startswith("docker buildx "), doc["command"]))
        canonical = members[0]
        # An explicit visibility restriction on this command also wins over
        # an alias's visible legacy definition (notably Buildx build flags).
        denied_options = {raw["option"] for raw in canonical.get("options", []) + canonical.get("inherited_options", [])
                          if not visible(raw)}
        usage = canonical.get("usage", canonical["command"])
        positional = usage.removeprefix(canonical["command"]).replace("[OPTIONS]", "").strip()
        spec = {"name": name.split()[-1], "description": text(canonical["short"]),
                "hint": "<string>" if positional else "", "subcommands": [], "options": []}
        if name in BUILD_CONTEXTS:
            spec.update(hint="<directory>", provider="directories")
        raw_options = {}
        for member in members:
            for raw in member.get("options", []) + member.get("inherited_options", []):
                if visible(raw) and raw["option"] not in denied_options:
                    raw_options.setdefault(raw["option"], raw)
        for option, raw in sorted(raw_options.items()):
            # A persistent ancestor supplies this spelling to descendants. An
            # alias outside that ancestor's branch receives its own copy.
            inherited = any((parent, option) in persistent for parent in ancestors(name))
            local = any(raw_local["option"] == option and visible(raw_local)
                        for raw_local in stable.get(name, {}).get("options", []))
            if inherited and not local:
                continue
            spec["options"].append(make_option(name, raw, (name, option) in persistent))
        specs[name] = spec

    for name in sorted(specs):
        if name == "docker":
            continue
        parent = name.rsplit(" ", 1)[0]
        if parent not in specs:
            raise ValueError(f"Missing documented parent {parent!r} of {name!r}")
        specs[parent]["subcommands"].append(specs[name])
    for name, spec in specs.items():
        popular = POPULAR.get(name, ())
        spec["subcommands"].sort(key=lambda child: (popular.index(child["name"]) if child["name"] in popular else len(popular), child["name"]))
        if spec["subcommands"]:
            spec["hint"] = ""
    return specs["docker"]


def walk(spec: dict, parent: str = ""):
    name = (parent + " " + spec["name"]).strip()
    yield name, spec
    for child in spec["subcommands"]:
        yield from walk(child, name)


def validate_source_coverage(spec: dict, documents: dict[str, dict]) -> None:
    """Check every original command/option against the generated tree."""
    entries = dict(walk(spec))
    for command, document in documents.items():
        excluded = any(any(documents.get(parent, {}).get(key, False) for key in EXCLUDED)
                       for parent in [command, *ancestors(command)])
        if excluded:
            if command in entries:
                raise ValueError(f"Excluded upstream command was included: {command}")
            continue
        if command not in entries:
            raise ValueError(f"Missing stable documented command: {command}")
        node = entries[command]
        local_names = {name for option in node["options"] for name in option["name"]}
        effective_names = local_names | {
            name for parent in ancestors(command) for option in entries[parent]["options"]
            if option["persistent"] for name in option["name"]
        }
        for raw in document.get("options", []) + document.get("inherited_options", []):
            spelling = "--" + raw["option"]
            if visible(raw) and spelling not in effective_names:
                raise ValueError(f"Missing stable documented option: {command} {spelling}")
            if not visible(raw) and spelling in local_names:
                raise ValueError(f"Excluded option reintroduced through alias: {command} {spelling}")


def validate(spec: dict) -> dict[str, int]:
    entries = dict(walk(spec))
    counts = {"commands": len(entries), "options": 0, "option_spellings": 0,
              "persistent_options": 0, "directory_options": 0, "file_options": 0, "directory_positionals": 0}
    for name, node in entries.items():
        if set(node) - {"name", "description", "hint", "provider", "subcommands", "options"}:
            raise ValueError(f"Unexpected Spec fields for {name}")
        if not all(isinstance(node[key], str) for key in ("name", "description", "hint")):
            raise ValueError(f"Invalid Spec text for {name}")
        children = [child["name"] for child in node["subcommands"]]
        if len(children) != len(set(children)):
            raise ValueError(f"Duplicate children under {name}")
        counts["directory_positionals"] += node.get("provider") == "directories"
        names = set()
        for option in node["options"]:
            if set(option) - {"name", "description", "hint", "provider", "values", "persistent"}:
                raise ValueError(f"Unexpected option fields under {name}")
            if names.intersection(option["name"]):
                raise ValueError(f"Conflicting option spelling under {name}: {option['name']}")
            names.update(option["name"])
            if not isinstance(option["persistent"], bool) or option["hint"] not in ("", "<string>", "<file>", "<directory>"):
                raise ValueError(f"Invalid option annotation under {name}")
            if option.get("provider", "") not in ("", "directories", "files"):
                raise ValueError(f"Invalid option provider under {name}")
            if "--workdir" in option["name"] and option.get("provider"):
                raise ValueError(f"Container --workdir must not use local paths under {name}")
            counts["options"] += 1
            counts["option_spellings"] += len(option["name"])
            counts["persistent_options"] += option["persistent"]
            counts["directory_options"] += option.get("provider") == "directories"
            counts["file_options"] += option.get("provider") == "files"

    def get(command: str, option: str) -> dict:
        return next(opt for opt in entries[command]["options"] if option in opt["name"])

    if get("docker", "--config").get("provider") != "directories":
        raise ValueError("Docker --config must complete directories")
    if any("--dir" in opt["name"] for opt in spec["options"]):
        raise ValueError("Docker has no documented root --dir option")
    if get("docker trust key generate", "--dir").get("provider") != "directories":
        raise ValueError("The actual documented trust key --dir option must be preserved")
    if get("docker compose", "--file").get("provider") != "files" or "-f" not in get("docker compose", "--file")["name"]:
        raise ValueError("Compose --file/-f must complete files")
    if get("docker compose", "--env-file").get("provider") != "files":
        raise ValueError("Compose --env-file must complete files")
    if get("docker compose", "--project-directory").get("provider") != "directories":
        raise ValueError("Compose --project-directory must complete directories")
    if get("docker compose up", "--detach")["hint"] or "-d" not in get("docker compose up", "--detach")["name"]:
        raise ValueError("Compose --detach/-d must be a boolean")
    if get("docker run", "--name")["hint"] != "<string>" or get("docker run", "--name").get("provider"):
        raise ValueError("Free string values must not request local-path suggestions")
    if any(opt["persistent"] for opt in spec["options"]):
        raise ValueError("Docker's documented public root flags are not inherited flags")
    if {opt["name"][0] for opt in entries["docker compose"]["options"] if opt["persistent"]} != {"--dry-run"}:
        raise ValueError("Only Compose --dry-run is documented as inherited")
    if counts != EXPECTED_COUNTS:
        raise ValueError(f"Reviewed output counts changed: {counts}; expected {EXPECTED_COUNTS}")
    return counts


def license_text(snapshots: dict[str, dict[str, bytes]]) -> bytes:
    docs, compose = SOURCES
    notice = f"""Docker completion documentation attribution
===========================================

docker.json is a modified, generated derivative of official Docker command
reference YAML. Kiln filters unstable/hidden/deprecated entries, joins documented
aliases, reformats descriptions, and adds local-path hints. No runtime source
download or Docker invocation is needed to use the generated catalog.

Docker documentation (Engine CLI and Buildx):
Copyright 2013-2026 Docker, Inc.
https://github.com/{docs.repo}/tree/{docs.commit}/data/cli
License: Apache-2.0. Upstream LICENSE is reproduced below.

Docker Compose:
https://github.com/{compose.repo}/tree/{compose.commit}/docs/reference
License: Apache-2.0. Upstream NOTICE and LICENSE are reproduced below.

--- docker/compose NOTICE (verbatim) ---
"""
    notice += snapshots["compose"]["NOTICE"].decode("utf-8")
    notice += "\n--- docker/docs LICENSE (verbatim) ---\n\n"
    notice += snapshots["docs"]["LICENSE"].decode("utf-8")
    notice += "\n--- docker/compose LICENSE (verbatim) ---\n\n"
    notice += snapshots["compose"]["LICENSE"].decode("utf-8")
    return notice.encode("utf-8")


def write_or_check(path: Path, data: bytes, check: bool) -> None:
    if path.is_file() and path.read_bytes() == data:
        return
    if check:
        raise ValueError(f"Generated file is missing or stale: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-dir", type=Path, default=DEFAULT_CACHE, help="External official source cache; default: %(default)s")
    parser.add_argument("--refresh", action="store_true", help="Explicitly download the fixed commits (does not advance pins)")
    parser.add_argument("--check", action="store_true", help="Compare generated JSON/license without writing them")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help="Spec JSON destination; sibling LICENSE-docker.txt is also checked/written")
    args = parser.parse_args()
    snapshots = {}
    for source in SOURCES:
        if args.refresh:
            refresh_source(source, args.cache_dir)
        snapshots[source.key] = read_source(source, args.cache_dir)
    documents = parse_documents(snapshots)
    catalog = generate(documents)
    validate_source_coverage(catalog, documents)
    counts = validate(catalog)
    output = (json.dumps(catalog, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    write_or_check(args.output, output, args.check)
    write_or_check(args.output.with_name("LICENSE-docker.txt"), license_text(snapshots), args.check)
    for source in SOURCES:
        print(f"{source.repo}@{source.commit}: {sum(n for _, n in source.directories)} YAML; SHA-256 {source.digest}")
    print(json.dumps(counts, sort_keys=True))
    print(f"{'Verified' if args.check else 'Generated'} {args.output}: {len(output)} bytes; SHA-256 {hashlib.sha256(output).hexdigest()}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, StopIteration, yaml.YAMLError) as error:
        sys.exit(f"Docker completion generation failed: {error}")
