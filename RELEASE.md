# Release checklist

Do every step on `main`, push it, and only then create the tag. The release build runs from the
tagged commit: it takes the version from Cargo.toml and the release notes from that commit's
CHANGELOG.md. A tag made before the prep is pushed ships the old version with empty notes
(v0.2.7 went out reporting 0.2.6 this way).

## 1. Before you start
- [ ] Every PR meant for this release is merged, and CI is green on `main`.
- [ ] `git checkout main && git pull`

## 2. Version number (all must match; `tools/check_versions.py` checks them)
- [ ] `Cargo.toml`: `version = "X.Y.Z"`
- [ ] `Cargo.lock`: updates on the next `cargo build`; commit it
- [ ] `installer/RadioMaster.iss`: `#define AppVersion "X.Y.Z"`

## 3. CHANGELOG.md
- [ ] Add a section at the top:
  ```
  ## vX.Y.Z — YYYY-MM-DD
  One-line summary of the release, with no full stop at the end

  **New**
  - …
  **Fixed**
  - …
  **Changed**
  - …
  ```
- [ ] The summary line is required. It becomes the "New in vX.Y.Z" line on the website's home page,
  the first line of the GitHub release notes, and the intro on the Updates page.
- [ ] Write entries for users: what changed for them, not how the code changed. Look through
  `git log vPREVIOUS..main` so nothing is missed.

## 4. Website (generated, never edit these parts by hand)
- [ ] `python3 tools/build_updates.py`. It updates:
  - `site/updates/index.html` (the Updates page)
  - the home page "New in vX.Y.Z" line (between `<!-- NEWS -->` markers in `site/index.html`)
  - `softwareVersion` in the home page's search-engine data
  - `<lastmod>` dates in `site/sitemap.xml`
- [ ] If pages or guides were added: `python3 tools/build_site.py`
- [ ] If a feature changes what the site says (requirements, screenshots, FAQ, docs), update those pages too.

## 5. Checks (the same ones CI runs)
- [ ] `cargo fmt`
- [ ] `cargo clippy --all-targets -- -D warnings` (with the Rust version pinned in `.github/workflows/ci.yml`)
- [ ] `cargo test`
- [ ] `python3 tools/check_versions.py vX.Y.Z` says "All versions are X.Y.Z"
- [ ] `python3 tools/extract_strings.py` if UI strings changed (commit `lang/template.json`)
- [ ] `python3 tools/check_glyphs.py` if icons or symbols changed
- [ ] Site links:
  ```
  mkdir -p site/img && cp docs/screenshots/*.png assets/icon.png docs/social-card.png site/img/
  python3 tools/check_site.py
  ```

## 6. Commit and push the prep
- [ ] `git commit -am "vX.Y.Z: version bump and changelog"` and `git push origin main`
- [ ] Wait for CI to pass on that commit.

## 7. Release notes
- [ ] Write `releases/vX.Y.Z-release-notes.md` in the project's shared folder, like the earlier ones:
  title, the summary, the CHANGELOG section, then the Downloads and Discord lines.
  (CI writes the GitHub release text from CHANGELOG.md on its own; this copy is for posting elsewhere.)

## 8. Tag (only after step 6 is pushed)
- [ ] From your own machine:
  ```
  git checkout main && git pull
  git tag -a vX.Y.Z -m vX.Y.Z
  git push origin vX.Y.Z
  ```
  Or create the release on the GitHub Releases page with tag `vX.Y.Z`, targeting `main`.
- [ ] The tag push builds the Windows installer and zip, the macOS app, the Linux tar.gz and the
  servers, attaches them to the release and replaces its text with the CHANGELOG section.

## 9. After the release
- [ ] Check the release page has all downloads and the right notes, and the website shows the new
  version on the home page and the Updates page.
- [ ] If the tag was made too early, move it to the prepped commit and push it again; CI rebuilds:
  ```
  git fetch origin && git tag -f -a vX.Y.Z origin/main -m vX.Y.Z && git push -f origin vX.Y.Z
  ```
