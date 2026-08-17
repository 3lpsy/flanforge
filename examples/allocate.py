#!/usr/bin/env python3
"""Request and release FlanForge macOS allocations from a Forgejo Actions job.

Every option falls back to an environment variable and is overridden by its
command-line flag. Standard library only, so it runs on any CI image.

Exit codes:
  0  success
  2  usage or environment error
  3  rejected by policy or authentication
  4  capacity is busy
  5  transport failure or timeout
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request

EXIT_USAGE = 2
EXIT_REJECTED = 3
EXIT_BUSY = 4
EXIT_TRANSPORT = 5

REPOSITORY = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$")
ALLOCATION_ID = re.compile(r"^[0-9a-fA-F-]{36}$")
MAX_BODY_BYTES = 65536


class Failure(Exception):
    """Aborts the command with a specific exit code."""

    def __init__(self, message: str, code: int) -> None:
        super().__init__(message)
        self.code = code


def env_default(name: str, fallback: str | None = None) -> str | None:
    value = os.environ.get(name, "").strip()
    return value or fallback


def request_json(
    url: str, method: str, token: str, timeout: float, body: dict | None = None
) -> dict:
    """Sends one authenticated request and decodes a bounded JSON response."""
    payload = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=payload, method=method)
    request.add_header("Authorization", f"Bearer {token}")
    request.add_header("Accept", "application/json")
    if payload is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw = response.read(MAX_BODY_BYTES)
    except urllib.error.HTTPError as error:
        detail = error.read(1024).decode("utf-8", "replace").strip()
        if error.code in (401, 403):
            raise Failure(f"rejected ({error.code}): {detail}", EXIT_REJECTED) from error
        if error.code == 409:
            raise Failure(f"busy ({error.code}): {detail}", EXIT_BUSY) from error
        raise Failure(f"request failed ({error.code}): {detail}", EXIT_TRANSPORT) from error
    except (urllib.error.URLError, TimeoutError) as error:
        raise Failure(f"cannot reach {url}: {error}", EXIT_TRANSPORT) from error
    if not raw:
        return {}
    try:
        decoded = json.loads(raw)
    except json.JSONDecodeError as error:
        raise Failure("response was not JSON", EXIT_TRANSPORT) from error
    if not isinstance(decoded, dict):
        raise Failure("response was not a JSON object", EXIT_TRANSPORT)
    return decoded


def identity_token(audience: str, timeout: float) -> str:
    """Mints a Forgejo Actions OIDC token for the configured audience."""
    url = env_default("ACTIONS_ID_TOKEN_REQUEST_URL")
    request_token = env_default("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
    if not url or not request_token:
        raise Failure(
            "no Actions identity endpoint; the job needs 'permissions: id-token: write'",
            EXIT_USAGE,
        )
    separator = "&" if "?" in url else "?"
    request = urllib.request.Request(f"{url}{separator}audience={audience}")
    request.add_header("Authorization", f"bearer {request_token}")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            decoded = json.loads(response.read(MAX_BODY_BYTES))
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
        raise Failure(f"cannot obtain an identity token: {error}", EXIT_TRANSPORT) from error
    value = decoded.get("value") if isinstance(decoded, dict) else None
    if not isinstance(value, str) or not value:
        raise Failure("identity endpoint returned no token", EXIT_TRANSPORT)
    return value


def emit_outputs(pairs: dict[str, str], path: str | None) -> None:
    """Writes step outputs when running inside Actions, and always to stdout."""
    for key, value in pairs.items():
        print(f"{key}={value}")
    if not path:
        return
    try:
        with open(path, "a", encoding="utf-8") as handle:
            for key, value in pairs.items():
                handle.write(f"{key}={value}\n")
    except OSError as error:
        raise Failure(f"cannot write outputs to {path}: {error}", EXIT_USAGE) from error


def require(value: str | None, flag: str, variable: str) -> str:
    if not value:
        raise Failure(f"{flag} is required (or set {variable})", EXIT_USAGE)
    return value


def base_url(value: str | None) -> str:
    url = require(value, "--url", "FLANFORGED_URL").rstrip("/")
    if not url.startswith(("http://", "https://")):
        raise Failure("--url must be an http(s) URL", EXIT_USAGE)
    return url


def allocate(arguments: argparse.Namespace) -> int:
    url = base_url(arguments.url)
    profile = require(arguments.profile, "--profile", "FLANFORGE_PROFILE")
    repository = require(arguments.repository, "--repository", "GITHUB_REPOSITORY")
    if not REPOSITORY.match(repository):
        raise Failure("--repository must be owner/name", EXIT_USAGE)
    for name, value in (("--run-id", arguments.run_id), ("--run-attempt", arguments.run_attempt)):
        if value < 1:
            raise Failure(f"{name} must be a positive integer", EXIT_USAGE)

    body = {
        "profile": profile,
        "repository": repository,
        "run_id": arguments.run_id,
        "run_attempt": arguments.run_attempt,
    }
    # Opt in to the project's warm image; omitted means the trusted base, cold,
    # which is what a release wants.
    if arguments.warm:
        body["warm"] = True
    for field, value in (("cpu_count", arguments.cpu_count), ("memory_mb", arguments.memory_mb)):
        if value is not None:
            body[field] = value
    # Capacity is bounded, so a request arriving while another job holds the
    # slot is answered busy at once. Waiting beats making someone re-trigger.
    deadline = time.monotonic() + max(arguments.wait, 0.0)
    while True:
        token = identity_token(arguments.audience, arguments.connect_timeout)
        try:
            allocation = request_json(
                f"{url}/v1/allocations", "POST", token, arguments.timeout, body
            )
            break
        except Failure as failure:
            remaining = deadline - time.monotonic()
            # Only busy is worth retrying; a rejection answers the same forever.
            if failure.code != EXIT_BUSY or remaining <= 0:
                raise
            pause = min(arguments.poll, remaining)
            print(
                f"busy; retrying in {pause:.0f}s ({remaining:.0f}s of budget left)",
                file=sys.stderr,
            )
            time.sleep(pause)
    label = allocation.get("runner_label")
    identifier = allocation.get("id")
    if not isinstance(label, str) or not isinstance(identifier, str):
        raise Failure("allocation response is missing its label or id", EXIT_TRANSPORT)
    print(f"allocated {identifier} in state {allocation.get('state', 'unknown')}", file=sys.stderr)
    emit_outputs({"runner_label": label, "allocation_id": identifier}, arguments.output)
    return 0


def cancel(arguments: argparse.Namespace) -> int:
    url = base_url(arguments.url)
    identifier = require(arguments.allocation_id, "--allocation-id", "FLANFORGE_ALLOCATION_ID")
    if not ALLOCATION_ID.match(identifier):
        raise Failure("--allocation-id must be an allocation UUID", EXIT_USAGE)
    token = identity_token(arguments.audience, arguments.connect_timeout)
    request_json(f"{url}/v1/allocations/{identifier}", "DELETE", token, arguments.connect_timeout)
    print(f"cancelled {identifier}", file=sys.stderr)
    return 0


def status(arguments: argparse.Namespace) -> int:
    url = base_url(arguments.url)
    identifier = require(arguments.allocation_id, "--allocation-id", "FLANFORGE_ALLOCATION_ID")
    if not ALLOCATION_ID.match(identifier):
        raise Failure("--allocation-id must be an allocation UUID", EXIT_USAGE)
    token = identity_token(arguments.audience, arguments.connect_timeout)
    allocation = request_json(
        f"{url}/v1/allocations/{identifier}", "GET", token, arguments.connect_timeout
    )
    print(json.dumps(allocation, indent=2, sort_keys=True))
    return 0


def positive_int(value: str) -> int:
    try:
        number = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if number < 1:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="allocate.py",
        description="Request and release FlanForge macOS allocations.",
        epilog="Environment variables supply defaults; flags override them.",
    )
    parser.add_argument(
        "--url",
        default=env_default("FLANFORGED_URL"),
        help="daemon base URL [FLANFORGED_URL]",
    )
    parser.add_argument(
        "--audience",
        default=env_default("FLANFORGE_AUDIENCE", "flanforged"),
        help="OIDC audience [FLANFORGE_AUDIENCE] (default: flanforged)",
    )
    parser.add_argument(
        "--connect-timeout",
        type=float,
        default=float(env_default("FLANFORGE_CONNECT_TIMEOUT", "30") or 30),
        help="seconds for short requests [FLANFORGE_CONNECT_TIMEOUT] (default: 30)",
    )
    parser.add_argument(
        "--output",
        default=env_default("GITHUB_OUTPUT"),
        help="step output file to append [GITHUB_OUTPUT]",
    )
    commands = parser.add_subparsers(dest="command", required=True)

    request = commands.add_parser("allocate", help="request an allocation and print its label")
    request.add_argument(
        "--profile", default=env_default("FLANFORGE_PROFILE"), help="profile name [FLANFORGE_PROFILE]"
    )
    request.add_argument(
        "--repository",
        default=env_default("GITHUB_REPOSITORY"),
        help="owner/name [GITHUB_REPOSITORY]",
    )
    request.add_argument(
        "--run-id",
        type=positive_int,
        default=env_default("GITHUB_RUN_ID", "0"),
        help="workflow run id [GITHUB_RUN_ID]",
    )
    request.add_argument(
        "--run-attempt",
        type=positive_int,
        default=env_default("GITHUB_RUN_ATTEMPT", "1"),
        help="workflow run attempt [GITHUB_RUN_ATTEMPT]",
    )
    request.add_argument(
        "--warm",
        action="store_true",
        default=bool(env_default("FLANFORGE_WARM")),
        help="ask for the project's warm image [FLANFORGE_WARM]",
    )
    request.add_argument(
        "--cpu-count",
        type=positive_int,
        default=env_default("FLANFORGE_CPU_COUNT"),
        help="guest CPUs, bounded by the profile [FLANFORGE_CPU_COUNT]",
    )
    request.add_argument(
        "--memory-mb",
        type=positive_int,
        default=env_default("FLANFORGE_MEMORY_MB"),
        help="guest memory, bounded by the profile [FLANFORGE_MEMORY_MB]",
    )
    request.add_argument(
        "--wait",
        type=float,
        default=float(env_default("FLANFORGE_WAIT", "0") or 0),
        help="seconds to keep retrying while capacity is busy [FLANFORGE_WAIT]",
    )
    request.add_argument(
        "--poll",
        type=float,
        default=float(env_default("FLANFORGE_POLL", "30") or 30),
        help="seconds between busy retries [FLANFORGE_POLL] (default: 30)",
    )
    # The daemon holds the request open while the guest boots.
    request.add_argument(
        "--timeout",
        type=float,
        default=float(env_default("FLANFORGE_TIMEOUT", "900") or 900),
        help="seconds to await the allocation [FLANFORGE_TIMEOUT] (default: 900)",
    )
    request.set_defaults(handler=allocate)

    for name, handler, help_text in (
        ("cancel", cancel, "release an allocation"),
        ("status", status, "print an allocation as JSON"),
    ):
        command = commands.add_parser(name, help=help_text)
        command.add_argument(
            "--allocation-id",
            default=env_default("FLANFORGE_ALLOCATION_ID"),
            help="allocation UUID [FLANFORGE_ALLOCATION_ID]",
        )
        command.set_defaults(handler=handler)

    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    if hasattr(arguments, "run_id"):
        arguments.run_id = int(arguments.run_id)
        arguments.run_attempt = int(arguments.run_attempt)
        for field in ("cpu_count", "memory_mb"):
            value = getattr(arguments, field)
            setattr(arguments, field, None if value is None else int(value))
    try:
        return int(arguments.handler(arguments))
    except Failure as failure:
        print(f"error: {failure}", file=sys.stderr)
        return failure.code


if __name__ == "__main__":
    sys.exit(main())
