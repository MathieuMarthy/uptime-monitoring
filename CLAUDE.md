# uptime-monitoring

Lightweight, self-hosted alternative to Uptime Kuma: internet-exposed, ~100 HTTP(S) monitors, minimal RAM,
single binary + SQLite, Docker image. Structure and conventions: `docs/architecture.md`; schema:
`docs/database-schema.md`.

## Commands

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

## Workflow

- **Tests first, without loading the TDD skill.** Write the failing test, watch it fail, implement, watch it pass.
  Do not invoke `superpowers:test-driven-development`; the discipline is enough.
- **Process sized to the work.** Medium sub-projects: a short design validated in chat, then implementation — no
  separate plan document. Full spec + plan only for large pieces (e.g. the check engine), and only when agreed.
- **One independent review per sub-project**, on the whole branch at the end — not per task.
- **No working documents in the repo after merge.** Specs and plans are deleted once their sub-project is merged;
  lasting knowledge goes into `docs/architecture.md` or `docs/database-schema.md`.
- **One session per sub-project.** State lives in the code, the docs and memory, not in the conversation.
- **Stay lean on tokens.** Load a guide or skill only when it is actually used. Redirect long command output to a
  file and read its tail. Do not re-send content already shown; describe only what changed. Batch doc updates.
