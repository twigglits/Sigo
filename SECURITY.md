# Security Policy

## Reporting a vulnerability

Sigo does NOT sandbox the Claude API calls (they are made with your own API key), but the
`--eval coding` benchmark **executes model-generated Python locally**. The binary ships with
an in-process sandbox that nulls common shell/file/exec entry points and caps address space.
When `bubblewrap` is available, the eval additionally runs each solution in a fresh set of
namespaces (no network, read-only system, private `/tmp`). On macOS, `sandbox-exec` is used
as a fallback (app sandbox with no networking, read-only file system, and private temp
directory).

Despite these precautions, the sandbox is **best-effort, not a security boundary**. For
genuinely untrusted corpora, run `sigo bench run --eval coding` inside a throwaway VM or
container.

## Reporting a vulnerability

If you discover a security issue — especially a sandbox escape in the eval runner, or a
leak of credentials (API keys, tokens) — please report it by email to the maintainer
(reachable via the GitHub profile at https://github.com/twigglits) rather than filing a
public issue.

I aim to acknowledge reports within 48 hours and ship a fix within a week for qualifying
issues.
