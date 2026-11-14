# Translating Mote

Mote's interface text is translated with gettext. Everything a translator
needs is in this directory; no Rust knowledge is required, and no source
file has to be touched. The languages Mote has, and who gave them, are in
[CREDITS.md](../docs/CREDITS.md#translations).

## Adding or updating a language

1. Take the template, `mote.pot`
   ([download](https://raw.githubusercontent.com/hyprlab/mote/main/po/mote.pot)).
   It lists every string the app shows, with an empty slot for the
   translation.
2. Create `<lang>.po` from it, where `<lang>` is the language code (`de`
   for German, `es` for Spanish, `pt_BR` for Brazilian Portuguese; add a
   region only when it matters):

       msginit --locale=de --input=mote.pot --output=de.po

   or open the template in [Poedit](https://poedit.net) or Gtranslator,
   pick the language when asked, and save it as `de.po`.
   To update an existing translation after the template changed, use the
   editor's "update from template" (or `msgmerge --update de.po mote.pot`);
   new strings show up untranslated and changed ones as "fuzzy".
3. Translate:
   - Keep the `{placeholders}` exactly as they are (they are filled in when
     the app runs), but move them around to suit the language.
   - Strings with two forms are plurals, like "1 connection" and "3
     connections". Give as many forms as the language uses; Poedit shows the
     right number of slots once the language is picked.
   - Labels for settings and buttons can be short. The longer strings are
     explanations under a setting.
   - Key names such as Ctrl+Alt+Delete can follow what the language's
     keyboards print. Protocol and product names (RDP, SSH, GNOME Remote
     Desktop, xrdp, KRdp) stay as they are.
   - Leave anything uncertain empty. An empty translation falls back to the
     English text, so a partial file is still useful.
4. Add the language code to `LINGUAS` (one per line) if it is new.
5. Check it compiles: `msgfmt --check de.po -o /dev/null`, or Poedit's
   Validate.

## Sending it in

Either way works, and neither needs Git experience beyond the first:

- **A pull request** against `main` that adds or changes `po/<lang>.po`
  (and `po/LINGUAS` for a new language). On GitHub, "Add file" in the `po`
  folder of a fork does it from the browser. Title it after the language,
  such as "German translation".
- **An attachment** on the
  [translations issue](https://github.com/hyprlab/mote/issues/2) or a new
  issue. GitHub only accepts certain file types, so rename the file to end
  in `.po.txt` first.

The maintainer takes it from there: merges it against the current template,
adds the language and its translator to the credits, and builds a beta so
it can be checked in the app. Credit goes to the GitHub account that sent
it, in the About window, [CREDITS.md](../docs/CREDITS.md) and the release
notes.

## Trying a translation in the app

From a source checkout:

    tools/build-locale.sh              # compiles po/*.po for a source-tree run
    LANG=de_DE.UTF-8 ./target/debug/mote

The app follows the desktop's language. The Flatpak, the RPM, the Debian
package and the AppImage all compile `po/*.po` into their own catalogues.

## For maintainers

`tools/update-pot.sh` regenerates `mote.pot` from the source (through
`xtr`, the Rust-aware xgettext: `cargo install xtr`) and refreshes every
`.po` against it. Run it whenever strings change and before a release, so
translators see the current set. In the code, user-facing strings go
through the helpers in `src/i18n.rs`: `i18n("...")`, `i18n_f` for named
placeholders, `ni18n_f` for plurals, and `i18n_noop` for tables of
literals translated where they are shown.

A translation is landed by hand rather than merged from its branch, which
is often made against an older `main`:

1. Take only the `.po` file (and the `LINGUAS` line), and merge it against
   the current template:
   `msgmerge --no-fuzzy-matching <file> mote.pot | msgattrib --no-obsolete -o <lang>.po`.
   Fuzzy guesses from older strings are usually wrong, so they are left
   untranslated instead.
2. `msgfmt --check --statistics <lang>.po -o /dev/null`, and run the app in
   the language.
3. Credit the translator in `data/TRANSLATORS` and in
   [CREDITS.md](../docs/CREDITS.md#translations).
4. Commit as `feat(i18n): add the <Language> translation (#<n>)`, or
   `fix(i18n): update the <Language> translation (#<n>)`, with the
   translator as the author:
   `git commit --author="<login> <<id>+<login>@users.noreply.github.com>"`
   (`gh api users/<login> --jq .id`). A `Co-Authored-By:` line is not
   enough: GitHub's contributor list counts authors only
   ([CONTRIBUTING.md](../docs/CONTRIBUTING.md#landing-a-contribution)).
5. Close the pull request with a comment: what was taken, what was changed
   on the way in, how many strings are left, and which beta carries it.
