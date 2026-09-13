# Release Operations Notes

中文：发版运营与下载数据口径

This page documents how to interpret GitHub release assets for MuxLayer. It is meant for maintainers looking at adoption, release quality, and user support signals.

## Download metrics

GitHub's total download count is useful as a rough activity signal, but it should not be treated as the number of real desktop installs.

MuxLayer releases include several asset types:

| Asset type | Examples | How to interpret |
|---|---|---|
| Desktop installers | `.dmg`, `.exe`, `.deb`, `.AppImage` | Closest proxy for user installs. Split by OS and architecture. |
| Headless CLI builds | `agentgate-serve-*.tar.gz`, `agentgate-serve-*.zip` | Server / Docker / advanced-user interest. Track separately from desktop installs. |
| Updater metadata | `latest.json` | In-app update checks and updater clients. Do not count as installs. |
| Signatures | `.sig` | Integrity metadata. Do not count as installs. |
| Source archives | GitHub auto-generated `.zip` / `.tar.gz` | Developer interest. Not an app install signal. |

Recommended reporting:

- Desktop installer downloads by platform.
- CLI downloads by platform.
- Updater metadata requests separately.
- Signature downloads excluded from adoption totals.
- Release-page traffic, stars, issues, and discussions reviewed alongside downloads.

## Release notes

The release workflow uses `scripts/extract-release-notes.mjs`.

The script prefers a curated bilingual file at:

```text
docs/release-notes/<version>.md
```

If that file does not exist, it falls back to the matching `CHANGELOG.md` section and generates bilingual section headings.

For important releases, add a curated release note before tagging so GitHub Releases has a concise English and Chinese summary.

## Frozen identifiers

The product is branded MuxLayer, but two identifiers keep their original AgentGate values and **must not be renamed**:

| Identifier | Value | What depends on it |
|---|---|---|
| Bundle identifier (`src-tauri/tauri.conf.json` → `identifier`) | `com.mengmian.agentgate` | App data directory (providers, tokens, request DB), macOS preferences / WebKit storage, updater identity, and the `zap` paths of both Homebrew casks |
| Updater endpoint (`plugins.updater.endpoints`) | `https://github.com/dengmengmian/muxlayer/releases/latest/download/latest.json` | Every installed client polls this URL; the updater public key in `tauri.conf.json` must stay paired with `TAURI_SIGNING_PRIVATE_KEY` |

Changing the identifier to something like `com.mengmian.muxlayer` makes existing installs look like a new app: settings and history appear lost, auto-update breaks, and `brew uninstall --zap` leaves the old data behind. Changing the endpoint strands every client already in the field.

## Version consistency

`scripts/check-version-consistency.sh` verifies that `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, the `agentgate` package in `src-tauri/Cargo.lock`, and the site HTML (`softwareVersion` + `data-version` spans in `site/index.html` and `site/zh/index.html`) carry the same version.

- CI (`frontend` job) runs it in local mode (package.json is the source of truth), so drift fails the PR.
- The release `preflight` job runs it first with the tag version (`v2.0.6` → `2.0.6`); `scripts/release-preflight.sh` runs it too.
- `scripts/publish-updater-json.mjs` refuses to publish `latest.json` if the tag differs from `tauri.conf.json`, because a mismatch makes installed clients offer the update forever.

Use `scripts/bump-version.sh <version>` to update all of these (plus the README / full-reference download links and the Homebrew cask `version` lines); it ends by running the consistency check.

## Release workflow (draft → publish)

`.github/workflows/release.yml` runs on a `v*` tag push:

1. `preflight` — version check against the tag, catalog / brand checks, offline fixture tests, Docker preflight.
2. `create-release` — creates the GitHub release **as a draft** (or reuses the existing one on re-run) and passes its id to later jobs. Drafts are not returned by `/releases/tags/{tag}`, so jobs address the release by id (or by `gh`, which resolves drafts by tag).
3. `build` — desktop bundles for macOS arm64 / x64, Linux and Windows are uploaded to the draft.
4. `updater-json` — merges the signatures into `latest.json` and uploads it to the draft (new file uploaded under a temporary name first, then the old `latest.json` is deleted and the new one renamed).
5. `cli` — headless `agentgate-serve` archives are uploaded to the draft.
6. `publish` — checks that `latest.json` is present, then runs `gh release edit <tag> --draft=false --latest`.
7. `homebrew-cask` — updates `muxlayer.rb` and `agentgate.rb` in the tap once the release is public.

If any platform build fails, the release stays a draft and `releases/latest/download/latest.json` keeps serving the previous stable release, so clients never hit a 404. Fix the problem and re-run the failed jobs; the draft is reused.

## Maintainer checklist

Before tagging:

- Run the full local release gate. This is the only accepted pre-release entrypoint; if Docker preflight cannot run, the release check is not complete.

```bash
pnpm test:release-local
```

- Confirm `README.md` and `README_ZH.md` point to the new installer filenames after release assets are known, or keep them pointing at the latest stable release intentionally.
- Confirm `CHANGELOG.md` has a version section.
- Add `docs/release-notes/<version>.md` for important releases.
- For non-release local debugging only, `AGENTGATE_SKIP_DOCKER_PREFLIGHT=1 pnpm test:release-local` may be used to isolate frontend/unit/quickstart failures.

After publishing:

- Check that the release is no longer a draft and is marked Latest.
- Check that installer assets, CLI assets, signatures, and `latest.json` uploaded successfully.
- Check that the release body is bilingual.
- Watch early issues for platform-specific install failures.
