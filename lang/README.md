# Translations

Every text in RadioMaster can be translated. A language is one JSON file in this folder, named after
the language as people would look for it, e.g. `Deutsch.json`, `Español.json`, `Nederlands.json`.

1. In RadioMaster: **Settings → General → Export translation template**. It writes a file with every
   text (and opens the folder). Or copy `template.json` from here.
2. Fill in the right-hand side of each line — leave a line empty to keep the English text:
   `"Add file…": "Datei hinzufügen…",`
3. Save it as `YourLanguage.json` in the `lang` folder of RadioMaster's data folder
   (`%APPDATA%\RadioMaster\lang`), then choose it under **Settings → General → Language**.
4. Share it: open a pull request adding your file to this folder, and add yourself to
   `CONTRIBUTORS.md` under Translations. It ships with the next release.

Developers: after adding new interface text (wrapped in `tr("…")`), run
`python3 tools/extract_strings.py` to refresh `template.json`.
