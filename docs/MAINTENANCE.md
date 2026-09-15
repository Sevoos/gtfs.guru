# Maintenance Guide: How to Safely Update GTFS Guru

This guide describes the workflow for making changes to the repository without breaking existing functionality.

## The Golden Rule

**Never push directly to `main`.** strict adherence to this rule ensures that the `main` branch is always stable and deployable.

---

## The Workflow

### 1. Create a Topic Branch

For every new feature or fix, start a new branch.

```bash
git checkout main
git pull                     # Get latest changes
git checkout -b my-new-feature # Create your branch
```

### 2. Make Your Changes

Edit files, write code.

### 3. Verify Locally (The "Safety Net")

Before you commit, run the checks locally.

```bash
# 1. Check for basic errors
cargo check

# 2. Run the test suite (CRITICAL)
cargo test --all

# 3. Check code style (Optional but recommended)
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

If `cargo test` fails, **do not commit**. Fix the errors first.

### 4. Commit and Push

```bash
git add .
git commit -m "feat: description of my awesome change"
git push -u origin my-new-feature
```

### 5. Create a Pull Request (PR)

1. Go to GitHub.
2. Click "Compare & pull request".
3. Create the PR.

**Wait for the "Checks" section.**
GitHub Actions will automatically run:

* ✅ Rust Tests (`cargo test`)
* ✅ Code Formatting
* ✅ Clippy Lints

**If the checks turn red ❌:**
Click "Details" to see what failed. Fix it locally, commit, and push again. The PR will update automatically.

**If the checks turn green ✅:**
You are safe! Click **"Squash and merge"**.

---

## Deploying the Website (gtfs.guru)

The live site **does not run on GitHub Pages** and is **not tied to a release
tag**. It is the `gtfs-guru-web` service (`crates/gtfs_validator_web`) with
`website/` embedded into the binary via
`include_dir!("$CARGO_MANIFEST_DIR/../../website")`, running as a Docker
container on the Hetzner VPS (`157.90.246.102`). Caddy on the host terminates
TLS for `gtfs.guru` and proxies to that container on `localhost:8080`. There is
no nginx and no directory of static files that Caddy serves directly: copying
`website/` to the server changes nothing until a new image is built and the
container is replaced.

**How it ships:** the `Deploy Web` workflow (`.github/workflows/deploy-web.yml`)
runs on every push to `main` that touches `website/`,
`crates/gtfs_validator_web/`, `crates/gtfs_validator_wasm/`, the `Dockerfile`
or the deploy script itself, and on demand via *Run workflow*. It

1. builds the image with `GIT_SHA` baked in, so `GET /version` reports
   `{"version": ..., "commit": "<sha>"}`;
2. streams it to the server over ssh (`docker save | zstd | ssh docker load`,
   no registry involved);
3. runs `deploy/swap-web-container.sh` there, which starts the new image with
   the running container's ports, env and volumes, waits for `/version` to
   report the new commit, and puts the previous image back if it does not;
4. checks `https://gtfs.guru/version` for that commit and runs the Playwright
   smoke test (`npm run test:live-website`).

Anything that only changes the validator core is *not* picked up by the path
filter; trigger the workflow manually, or refresh `website/pkg` with
`./scripts/build-wasm.sh` and commit it, which also updates the in-browser
validator.

**Manual path** (needs ssh as `botuser`): build the image locally for
`linux/amd64`, ship it the same way, and run the swap script:

```bash
docker build --platform linux/amd64 --build-arg GIT_SHA=$(git rev-parse HEAD) -t gtfs-validator-web:$(git rev-parse --short HEAD) .
docker save gtfs-validator-web:$(git rev-parse --short HEAD) | zstd | ssh botuser@157.90.246.102 'zstd -d | docker load'
ssh botuser@157.90.246.102 "bash -s -- gtfs-validator-web:$(git rev-parse --short HEAD) $(git rev-parse HEAD)" < deploy/swap-web-container.sh
```

Notes:

* The repo-root `website/` is the **single copy** of the site. `gtfs-guru-web`
  embeds it, which is why the crate is `publish = false`: `cargo package`
  cannot carry a directory from outside the crate root, and the crate is a
  deployed binary rather than a library anyone depends on.
* `docker-compose.yml` is the local/self-hosting stack. Production does not use
  it; the container was started with `docker run`, and the swap script
  preserves whatever it was started with. `deploy/update.sh` rebuilds the
  compose stack and is **not** what serves the live domain.
* The example feed behind the "Try an example feed" button is generated, not
  hand-edited. Change `scripts/build_demo_feed.py` and re-run it
  (`python3 scripts/build_demo_feed.py`); `--check` is what CI runs.
* Notice documentation is generated from the Rust schema and
  `src/notice_guides.json`. Run `cargo run -p gtfs-guru-web --bin generate-notice-pages`
  after changing a notice or guide. The same command writes
  `website/compatibility/`, `website/sitemap.xml` and `docs/rules.md`; CI runs
  it with `-- --check` and fails when the committed output is stale. Refresh the
  bundled MobilityData snapshot with `python3 scripts/update_notice_metadata.py`;
  normal builds never require network access.
* Server-level config (headers, TLS, caching) lives in the host's
  `/etc/caddy/Caddyfile` (root-owned; the repo `Caddyfile` is the compose
  variant). COOP/COEP for multithreaded WASM are set there.

---

## Releasing a New Version

A release is intentionally gated by a `v*` tag. Merging to `main` or running the
workflow manually only builds artifacts; it does not publish or deploy anything.

1. Update every package version and the Tauri version.
2. Run `python3 scripts/check-release-version.py --tag vX.Y.Z`.
3. Run the normal Rust, golden, WASM, and browser checks and merge to `main`.
4. Only after explicit release approval, push the matching `vX.Y.Z` tag.
5. Move the major-version tag the GitHub Action is published under, so that
   `abasis-ltd/gtfs.guru/action@v1` keeps resolving to the newest release:

   ```bash
   git tag -f v1 vX.Y.Z && git push -f origin v1
   ```

   Without this step every workflow pinned to `@v1` keeps running the previous
   release, and a brand-new major tag does not exist at all.

The tag workflow verifies version consistency before it does any build. It then:

* builds desktop installers and CLI archives for macOS, Linux, and Windows;
* creates the GitHub Release and updater manifest;
* publishes the Rust crates, Python wheel, and npm package.

The website is not part of the tag: see
[Deploying the Website](#deploying-the-website-gtfsguru).

Required release secrets are `CARGO_REGISTRY_TOKEN`, `PYPI_API_TOKEN`,
`NPM_TOKEN` and the Tauri/Apple signing secrets. The web deploy uses
`HETZNER_HOST`, `HETZNER_SSH_KEY`, `HETZNER_KNOWN_HOSTS` and (optionally)
`HETZNER_USER`, default `botuser`.

The known-hosts value must be provisioned out of band (for example from a
trusted existing SSH connection). Neither workflow uses `ssh-keyscan`.
