# Contributing

Contributions are welcome, and not only in Rust. A report that something is
wrong, awkward or missing is worth as much as a patch.

## Ways in

- **Report a bug:** [open an issue](https://github.com/hyprlab/mote/issues).
  Say which version of Mote, which distribution, and which RDP server the
  remote machine runs (GNOME Remote Desktop, xrdp, KRdp, and its version).
- **Suggest a feature:** an issue describing what you are trying to do, rather
  than the control you imagine, gets the best result.
- **Translate:** no Rust needed, and everything a translator wants is in
  [po/README.md](../po/README.md), from the template to sending the file in.
- **Design and UX:** GNOME HIG corrections, layout and wording feedback are
  welcome as issues.
- **Code:** send a pull request; see below. Mote is built in the sections
  listed in [ROADMAP.md](ROADMAP.md), which says what each holds and what is
  still to come.

## Pull requests

- Branch from `main` and keep the change to one subject.
- Build it and run it before opening the PR (`cargo build`, then the app).
  [BUILDING.md](BUILDING.md) has the dependencies.
- Match the surrounding code: the codebase comments *why*, not *what*, and
  user-facing strings go through `i18n()`.
- There's no CLA. By opening a pull request you agree your contribution ships
  under the [AGPL-3.0-or-later](../LICENSE), and that it may be adapted before
  it lands, with the change explained on the pull request.

## Two platforms

Mote is a GNOME app first, and every release also ships for macOS from the
same code. A change for one platform must leave the other exactly as it was:

- What only one platform does lives in a module of its own, compiled for
  that platform alone with `#[cfg(target_os = "macos")]` or
  `#[cfg(not(target_os = "macos"))]`, such as `src/secrets/keychain.rs`
  beside `src/secrets/secret_service.rs`, or `src/rdp/keymap/mac.rs`.
- Shared code asks about the platform only at small seams: a `cfg!` choosing
  between two strings, or a helper in `src/platform.rs`, such as the one that
  makes `<Control>` shortcuts Command on macOS.
- A platform's dependencies go under
  `[target.'cfg(...)'.dependencies]` in `Cargo.toml`.
- A user-facing string that differs gets its own msgid on each platform,
  rather than one string with the platform's word filled in.
- Every push is built, linted and tested on both by
  `.github/workflows/check.yml`; [BUILDING.md](BUILDING.md#macos) has the
  macOS build.

## Commits

Subjects follow [Conventional Commits](https://www.conventionalcommits.org):
`type(area): summary`, such as `fix(clipboard): keep line endings in pasted
text`. The type is one of `feat`, `fix`, `perf`, `refactor`, `docs`, `build`,
`ci`, `test`, `style`, `chore` or `revert`; the area is the part of the app the
change is in. Keep the subject to 72 characters, lower case after the colon,
with no full stop.

The body is optional. When there is one, it says in a few lines why the change
exists, in factual language with no marketing, no emoji and no em dashes, and
stays under 100 words. Write each paragraph as a single line: GitHub shows a
commit body with its line breaks kept, so a body wrapped by hand breaks again
wherever the screen is narrower than the wrap.

Trailers are for people: a `Co-Authored-By:` line credits somebody whose work
is in the commit, using the `users.noreply.github.com` address that resolves to
their profile, and not for the tools anyone wrote it with. The README's
[AI notice](../README.md#ai-notice) covers that for the repository as a whole.
`tools/git-hooks/commit-msg` checks all of this; point a clone at the hooks with
`git config core.hooksPath tools/git-hooks`.

## Landing a contribution

A pull request is usually landed by hand rather than merged, since its branch
is often made against an older `main`, but the commit that lands it is still
the contributor's: `git commit --author="<login>
<<id>+<login>@users.noreply.github.com>"`, with the id from `gh api
users/<login> --jq .id`, and the maintainer as committer. GitHub's contributor
list counts commit authors only, so a `Co-Authored-By:` line on a commit of
the maintainer's leaves the contributor out of it, and that can't be put right
after a release without rewriting history. Changes of the maintainer's own on
the way in go in a commit of their own.

Everyone in `data/CONTRIBUTORS` and `data/TRANSLATORS` has to be the author of
a commit: `tools/check-credits.sh` checks it, `tools/git-hooks/pre-push`
refuses a push without it, and CI runs it on every push.

## Documentation

Documentation lives in `docs/`, one subject per file, and the
[README](../README.md) is the front door: what the app is, its goals, how to
build it, and links. A change that documents something goes where that
something already lives, and a new page belongs in
[docs/README.md](README.md)'s index. Artwork for the README or the docs goes in
`data/repo/`; `docs/` holds Markdown only.

[RELEASE_NOTES.md](RELEASE_NOTES.md) is what the About window shows under
Release Notes and what a GitHub release says: under "What's new in" a
version, one-line items grouped as `### New features:`, `### Fixes:` and
`### Translations:`, in that order, with no bold. The people the About
window thanks are listed in `data/CONTRIBUTORS` (name and GitHub handle)
and `data/TRANSLATORS` (the same, then the languages after a dash), which
arrive with their first names; what each of them did is in
[CREDITS.md](CREDITS.md).

The prose is factual and plain: no marketing, no emoji, and no em dashes,
which a colon, a comma or a full stop replaces.
`python3 tools/check-docs.py` checks all of this, every relative link and
anchor included. Run it after touching any `.md`.

## AI-assisted contributions

Mote is built with AI assistance itself (see the
[AI notice](../README.md#ai-notice)), so patches written with AI tools are as
welcome as any other. Everything merged gets the same human review, and the
same rule applies either way: you are responsible for what your patch does.
