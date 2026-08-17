#!/usr/bin/env node
// Request and release FlanForge macOS allocations from a Forgejo Actions job.
//
//   allocator.js allocate            request an allocation, print its label
//   allocator.js status <id>         print one allocation as JSON
//   allocator.js cancel <id>         release an allocation
//
// Every option falls back to an environment variable and is overridden by its
// flag. CommonJS on purpose: a `.js` file is CommonJS wherever no package.json
// declares otherwise, so this runs unchanged in any repository. Node 18+ for
// global fetch; no dependencies.
//
// The workflow must set `enable-openid-connect: true`. Forgejo does not
// implement GitHub's `permissions: id-token: write`, and without the Forgejo
// key no identity endpoint is injected.
//
// Exit codes: 2 usage, 3 rejected, 4 busy, 5 transport.
"use strict";

const { appendFileSync } = require("node:fs");

const EXIT_USAGE = 2;
const EXIT_REJECTED = 3;
const EXIT_BUSY = 4;
const EXIT_TRANSPORT = 5;

const REPOSITORY = /^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*$/;
const ALLOCATION_ID = /^[0-9a-fA-F-]{36}$/;
const MAX_BODY_BYTES = 65536;

class Failure extends Error {
  constructor(message, code) {
    super(message);
    this.code = code;
  }
}

const env = (name, fallback) => (process.env[name] ?? "").trim() || fallback;

const truthy = (value) => value === true || ["1", "true", "yes"].includes(String(value ?? "").toLowerCase());

// fetch rejects with `TypeError: fetch failed`; the useful part is the cause.
const reason = (error) => error?.cause?.message ?? error?.message ?? String(error);

// Flags that take no value; everything else requires one.
const BOOLEAN_FLAGS = new Set(["warm"]);

function parseArguments(argv) {
  const parsed = { _: [] };
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === "-h" || value === "--help") {
      parsed.help = true;
    } else if (value.startsWith("--")) {
      const name = value.slice(2);
      if (BOOLEAN_FLAGS.has(name)) {
        parsed[name] = true;
        continue;
      }
      const next = argv[index + 1];
      if (next === undefined || next.startsWith("--")) {
        throw new Failure(`${value} requires a value`, EXIT_USAGE);
      }
      parsed[name] = next;
      index += 1;
    } else {
      parsed._.push(value);
    }
  }
  return parsed;
}

function number(value, name, { min = 1 } = {}) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed < min) {
    throw new Failure(`${name} must be a number of at least ${min}`, EXIT_USAGE);
  }
  return parsed;
}

function required(value, flag, variable) {
  if (!value) throw new Failure(`${flag} is required (or set ${variable})`, EXIT_USAGE);
  return value;
}

function baseUrl(value) {
  const url = required(value, "--url", "FLANFORGED_URL").replace(/\/+$/, "");
  if (!/^https?:\/\//.test(url)) throw new Failure("--url must be an http(s) URL", EXIT_USAGE);
  return url;
}

function allocationId(value) {
  const id = required(value, "--allocation-id", "FLANFORGE_ALLOCATION_ID");
  if (!ALLOCATION_ID.test(id)) throw new Failure("allocation id must be a UUID", EXIT_USAGE);
  return id;
}

// Reads a bounded body so a hostile or broken response cannot exhaust memory.
async function readBounded(response) {
  const text = await response.text();
  if (text.length > MAX_BODY_BYTES) throw new Failure("response was too large", EXIT_TRANSPORT);
  return text;
}

async function send(url, { method, token, timeout, body }) {
  let response;
  try {
    response = await fetch(url, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/json",
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(timeout * 1000),
    });
  } catch (error) {
    // Node wraps the real reason in `cause`; `TypeError: fetch failed` alone
    // tells an operator nothing. The token lives in a header, not the URL.
    throw new Failure(`cannot reach ${url}: ${reason(error)}`, EXIT_TRANSPORT);
  }
  const text = await readBounded(response);
  if (!response.ok) {
    // Collapse to one line: an error page arrives as multi-line HTML.
    const detail = text.replace(/\s+/g, " ").trim().slice(0, 200);
    if (response.status === 401 || response.status === 403) {
      throw new Failure(`rejected (${response.status}): ${detail}`, EXIT_REJECTED);
    }
    if (response.status === 409) {
      throw new Failure(`busy (${response.status}): ${detail}`, EXIT_BUSY);
    }
    throw new Failure(`request failed (${response.status}): ${detail}`, EXIT_TRANSPORT);
  }
  if (!text) return {};
  try {
    const decoded = JSON.parse(text);
    if (decoded === null || typeof decoded !== "object") {
      throw new Failure("response was not a JSON object", EXIT_TRANSPORT);
    }
    return decoded;
  } catch (error) {
    if (error instanceof Failure) throw error;
    throw new Failure("response was not JSON", EXIT_TRANSPORT);
  }
}

async function identityToken(audience, timeout) {
  const url = env("ACTIONS_ID_TOKEN_REQUEST_URL");
  const requestToken = env("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
  if (!url || !requestToken) {
    throw new Failure(
      "no Actions identity endpoint; the workflow needs 'enable-openid-connect: true'",
      EXIT_USAGE,
    );
  }
  const separator = url.includes("?") ? "&" : "?";
  let response;
  try {
    response = await fetch(`${url}${separator}audience=${encodeURIComponent(audience)}`, {
      headers: { Authorization: `bearer ${requestToken}` },
      signal: AbortSignal.timeout(timeout * 1000),
    });
  } catch (error) {
    throw new Failure(`cannot obtain an identity token: ${reason(error)}`, EXIT_TRANSPORT);
  }
  if (!response.ok) {
    throw new Failure(`identity endpoint returned ${response.status}`, EXIT_TRANSPORT);
  }
  const value = JSON.parse(await readBounded(response))?.value;
  if (typeof value !== "string" || !value) {
    throw new Failure("identity endpoint returned no token", EXIT_TRANSPORT);
  }
  return value;
}

function emit(pairs) {
  for (const [key, value] of Object.entries(pairs)) console.log(`${key}=${value}`);
  const path = env("GITHUB_OUTPUT");
  if (!path) return;
  try {
    const lines = Object.entries(pairs).map(([key, value]) => `${key}=${value}\n`);
    appendFileSync(path, lines.join(""));
  } catch (error) {
    throw new Failure(`cannot write outputs to ${path}: ${error.code}`, EXIT_USAGE);
  }
}

const sleep = (seconds) => new Promise((resolve) => setTimeout(resolve, seconds * 1000));

async function allocate(options) {
  const url = baseUrl(options.url ?? env("FLANFORGED_URL"));
  const audience = options.audience ?? env("FLANFORGE_AUDIENCE", "flanforged");
  const connectTimeout = number(options["connect-timeout"] ?? env("FLANFORGE_CONNECT_TIMEOUT", "30"), "--connect-timeout");
  const repository = required(
    options.repository ?? env("GITHUB_REPOSITORY"),
    "--repository",
    "GITHUB_REPOSITORY",
  );
  if (!REPOSITORY.test(repository)) throw new Failure("--repository must be owner/name", EXIT_USAGE);
  const body = {
    profile: required(options.profile ?? env("FLANFORGE_PROFILE"), "--profile", "FLANFORGE_PROFILE"),
    repository,
    run_id: number(options["run-id"] ?? env("GITHUB_RUN_ID"), "--run-id"),
    run_attempt: number(options["run-attempt"] ?? env("GITHUB_RUN_ATTEMPT", "1"), "--run-attempt"),
  };
  // Opt in to the project's warm image. Absent means the trusted base, cold,
  // which is what a release wants.
  if (truthy(options.warm ?? env("FLANFORGE_WARM"))) body.warm = true;
  for (const [flag, field] of [["cpu-count", "cpu_count"], ["memory-mb", "memory_mb"]]) {
    const value = options[flag] ?? env("FLANFORGE_" + field.toUpperCase());
    if (value !== undefined) body[field] = number(value, "--" + flag);
  }
  // The daemon holds the request open while the guest boots, so this timeout
  // must exceed the profile's boot budget.
  const timeout = number(options.timeout ?? env("FLANFORGE_TIMEOUT", "900"), "--timeout");
  // Capacity is bounded, so a request arriving while another job holds the slot
  // is answered busy at once. Waiting beats making someone re-trigger.
  const wait = number(options.wait ?? env("FLANFORGE_WAIT", "0"), "--wait", { min: 0 });
  const poll = number(options.poll ?? env("FLANFORGE_POLL", "30"), "--poll");

  const deadline = Date.now() + wait * 1000;
  let allocation;
  for (;;) {
    const token = await identityToken(audience, connectTimeout);
    try {
      allocation = await send(`${url}/v1/allocations`, { method: "POST", token, timeout, body });
      break;
    } catch (error) {
      const remaining = (deadline - Date.now()) / 1000;
      // Only busy is worth retrying; a rejection answers the same forever.
      if (!(error instanceof Failure) || error.code !== EXIT_BUSY || remaining <= 0) throw error;
      const pause = Math.min(poll, remaining);
      console.error(`busy; retrying in ${pause.toFixed(0)}s (${remaining.toFixed(0)}s of budget left)`);
      await sleep(pause);
    }
  }

  const { runner_label: label, id, state } = allocation;
  if (typeof label !== "string" || typeof id !== "string") {
    throw new Failure("allocation response is missing its label or id", EXIT_TRANSPORT);
  }
  console.error(`allocated ${id} in state ${state ?? "unknown"}`);
  emit({ runner_label: label, allocation_id: id });
}

async function simple(options, method, render) {
  const url = baseUrl(options.url ?? env("FLANFORGED_URL"));
  const audience = options.audience ?? env("FLANFORGE_AUDIENCE", "flanforged");
  const timeout = number(options["connect-timeout"] ?? env("FLANFORGE_CONNECT_TIMEOUT", "30"), "--connect-timeout");
  const id = allocationId(options["allocation-id"] ?? options._[1] ?? env("FLANFORGE_ALLOCATION_ID"));
  const token = await identityToken(audience, timeout);
  render(id, await send(`${url}/v1/allocations/${id}`, { method, token, timeout }));
}

const usage = `Usage: allocator.js <allocate|status|cancel> [options]

  --url URL              daemon base URL [FLANFORGED_URL]
  --profile NAME         profile to request [FLANFORGE_PROFILE]
  --repository OWNER/NAME    [GITHUB_REPOSITORY]
  --run-id N                 [GITHUB_RUN_ID]
  --run-attempt N            [GITHUB_RUN_ATTEMPT] (default 1)
  --allocation-id UUID   for status and cancel [FLANFORGE_ALLOCATION_ID]
  --audience NAME        OIDC audience [FLANFORGE_AUDIENCE] (default flanforged)
  --warm                 ask for the project's warm image [FLANFORGE_WARM]
  --cpu-count N          guest CPUs, bounded by the profile [FLANFORGE_CPU_COUNT]
  --memory-mb N          guest memory, bounded by the profile [FLANFORGE_MEMORY_MB]
  --wait SECONDS         keep retrying while busy [FLANFORGE_WAIT] (default 0)
  --poll SECONDS         between busy retries [FLANFORGE_POLL] (default 30)
  --timeout SECONDS      allocation request [FLANFORGE_TIMEOUT] (default 900)
  --connect-timeout SECONDS  short requests [FLANFORGE_CONNECT_TIMEOUT] (default 30)`;

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const command = options._[0];
  if (options.help || !command) {
    console.log(usage);
    return command ? 0 : EXIT_USAGE;
  }
  switch (command) {
    case "allocate":
      await allocate(options);
      return 0;
    case "status":
      await simple(options, "GET", (_id, body) => console.log(JSON.stringify(body, null, 2)));
      return 0;
    case "cancel":
      await simple(options, "DELETE", (id) => console.error(`cancelled ${id}`));
      return 0;
    default:
      throw new Failure(`unknown command '${command}'`, EXIT_USAGE);
  }
}

main()
  .then((code) => process.exit(code ?? 0))
  .catch((error) => {
    console.error(`error: ${error.message}`);
    process.exit(error instanceof Failure ? error.code : EXIT_TRANSPORT);
  });
