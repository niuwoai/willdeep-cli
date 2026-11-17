# Browser bridge 0.151.0-rc1

- Result: failed
- Fixture: http://127.0.0.1:19851
- Started: 2026-11-16T23:43:08Z

| Test | Result | Detail |
| --- | --- | --- |
| isolated_profile_clean_environment_and_dependencies | failed | "Bridge result mismatch: {\"code\"=>\"UNKNOWN\", \"message\"=>\"Chrome CDP startup timed out\", \"retryable\"=>false}" |

## Boundaries

- Real Node/Chrome CDP bridge; Rust host and shared execute/approval not compiled or executed by this script.
- Navigation/request origin controls and Chrome background flags; not whole-process network isolation.
- One disposable profile per process, clean environment; no dependency installation or external service.

## Not verified

- Rust compilation/unit tests
- Runtime approval and host ref binding
- Windows process job
- OS-level network isolation
