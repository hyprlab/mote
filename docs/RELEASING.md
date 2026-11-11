# Releasing

What a Mote release page says, which tags stay, and how the macOS app ships. The build and the package
steps are in [BUILDING.md](BUILDING.md).

## The release page

A release page is the version's section of
[RELEASE_NOTES.md](RELEASE_NOTES.md) and a compare link, nothing else.
`tools/release-notes.sh` writes both:

```sh
tools/release-notes.sh 1.3.0 > temp/release/1.3.0-github.md
```

The link compares from a tag that stays, so it keeps working: a beta from the
last stable, a stable from the last tag of the series before its own.

## Tags

A beta's tag and release page go once the next beta supersedes them, after its
release page and notes are up. Stable tags stay.

A dropped tag kills every compare link naming it. After dropping any tag, run
`tools/check-release-links.sh --fix`, which finds the pages still naming a tag
that is gone and points them at the tag `tools/release-notes.sh` picks.

## The macOS app

Every release ships the macOS app beside the Linux packages. The release
workflow's macOS job builds `Mote-<version>.dmg` (`Mote-Beta-<version>.dmg`
for a beta) on GitHub's oldest Apple silicon runner, signed with Mote's own
certificate from the repository's secrets `MOTE_MACOS_CERTIFICATE` (the
`.p12` file in base64) and `MOTE_MACOS_CERTIFICATE_PASSWORD`. It runs the app
from the disk image with Homebrew moved away before the release is drafted,
and a tag without the certificate fails.

Publishing it is uploading the disk image to Mote's Flatout as a package, on
the release's channel, with the release's notes: Flatout signs it with its
Sparkle key and adds it to the appcast, and installed copies update to it.

Two keys stand behind every installed copy, and neither can be replaced
without cost:

- **The signing certificate**, kept by the maintainer and in the secrets. A
  build signed with another is a different app to macOS: the Keychain asks
  again for every saved password, and Sparkle refuses it as an update, so
  everyone would have to download the new version by hand.
- **The Sparkle key**, kept by Flatout, whose public half is in every copy
  (`tools/build-macos-app.sh`). Back it up from Flatout's Repository >
  Signing and addresses. Copies only take updates signed with it.
