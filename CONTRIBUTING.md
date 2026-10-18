# Contributing to AgentPlus

Bug fixes, new provider templates, translations and support for new agent releases are all
welcome. For a larger change (a new page, a new agent, a change to how keys are stored or
sent), open an issue first so we can agree on the approach before you write it.

## Setup

You'll need Bun 1.3+, Node.js 18+, Rust 1.88+ and the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

```bash
bun install
bun run tauri dev
```

`bun run dev` previews the interface in a browser with demo data.

## Before you open a pull request

Run the same checks as CI (it runs them on Windows and macOS):

```bash
bun run check
cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo test
```

Check your commit subjects (CI checks them, and the pull request title, too):

```bash
node scripts/check-commits.mjs origin/main HEAD
```

## Rules

[AGENTS.md](AGENTS.md) has the full rules. The ones pull requests most often miss:

- **Commit subjects** are `type(scope): description`, e.g. `fix(gateway): keep tool call ids`:
  lower-case kebab-case type and scope, imperative description, no trailing period, at most
  72 characters. One change per commit. No `Co-Authored-By` or other AI attribution.
- **Both languages.** No user-visible text is hard-coded. Add every string to
  `src/i18n/en/` and `src/i18n/zh/` (frontend) or wrap it in `l()` / `tr!` (backend). If you
  don't write Chinese, say so in the pull request and the maintainer will fill it in.
- **Keep older agent releases working.** Support for a new Codex or Claude Code release is
  added next to the old handling, picked by version number, never in its place.
- **Keys stay with their provider.** A stored key is only ever sent to the host it was saved
  for. Changes to where requests go get the closest review.
- **Tests.** Logic changes come with unit tests for the edge cases.
- Code, comments and docs are in English.

## Pull requests

- Branch from `main` and keep one topic per pull request.
- Leave **Allow edits by maintainers** on, so small fixes can be pushed to your branch
  instead of going back and forth.
- Don't change the version number or `CHANGELOG.md`; both are updated at release time.
- Workflows on pull requests from forks run after a maintainer approves them.

## License

AgentPlus is released under [AGPL-3.0-only](LICENSE) and is also offered under a commercial
license. By submitting a contribution, you agree that it is licensed under AGPL-3.0-only and
that the copyright holder may also distribute it under other license terms, including that
commercial license. You keep the copyright to your contribution.
