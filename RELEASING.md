# Releasing Zarra

```sh
cargo info zarrs@0.23.15
```

**Run the upstream gate first.** It must succeed before any `cargo package` (other than the file-only `--list` check) or `cargo publish` command. If it is unavailable or fails, stop: retain the local/path release only. A full-SHA git backport is not registry availability.

Only after that command succeeds may an owner publish version 0.1.0. The release order is `zarra` core, then `zarra-cli`; there is no separate SQL crate. The standalone `zarra-wasm` package is source-built with wasm-pack and is not another registry release step. These are owner-run future steps and were not executed as part of the local/path packaging checks.

## 1. Require a clean tree and complete verification

Confirm `git status --short` is empty on the intended `main` commit, then run:

```sh
bash scripts/verify-fixtures.sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo check -p zarra --no-default-features
cargo test -p zarra-wasm --locked
cargo clippy -p zarra-wasm --target wasm32-unknown-unknown --locked -- -D warnings
npx --yes wasm-pack@0.15.1 build crates/zarra-wasm --target web --release -- --locked
npx --yes wasm-pack@0.15.1 test --headless --chrome crates/zarra-wasm -- --locked
node --test crates/zarra-wasm/tests/browser.test.mjs
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps
cargo +1.91.0 test --workspace --all-features --locked
npm --prefix example test
install_root="$(mktemp -d)"
cargo install --path crates/zarra-cli --root "$install_root"
"$install_root/bin/zarra" --help
rm -rf "${install_root:?}"
cargo package -p zarra --list --allow-dirty
cargo package -p zarra-cli --list --allow-dirty
bash scripts/test-gdal.sh
```

Every command must pass. The core list must include its README and test fixture. The CLI list must include its README, all embedded assets, and all three vendored MapLibre files. `--list` verifies inclusion only; it does not establish publish readiness.

## 2. Recheck both crate names

```sh
cargo info zarra
cargo info zarra-cli
```

For this initial release, both commands should report that the crate does not exist. If either name is already registered or owned by someone else, stop and resolve the name/ownership issue before packaging. The earlier availability check is not a permanent reservation.

## 3. Package and publish the core

```sh
cargo package -p zarra
cargo publish -p zarra
```

Do not package the CLI yet. Its path dependency also declares `zarra = "0.1.0"`, so the core must be visible in the registry before CLI packaging can succeed.

## 4. Wait for the core in the index

```sh
cargo info zarra@0.1.0
```

Retry until this succeeds and shows the newly published core. Only then continue:

```sh
cargo package -p zarra-cli
cargo publish -p zarra-cli
```

## 5. Tag the published commit

```sh
git tag -a v0.1.0 -m "zarra v0.1.0"
git push origin v0.1.0
```

## 6. Verify docs.rs

Confirm that the docs.rs build for `zarra 0.1.0` succeeds with the `async`, `http`, and `sql` features selected by `[package.metadata.docs.rs]`, and that `RasterStore::open_url`, `AsyncRasterStore`, and the shared SQL API appear in the documentation. Check the destination over HTTP:

```sh
curl --fail --head https://docs.rs/zarra/0.1.0/zarra/
curl --fail --silent --show-error https://docs.rs/crate/zarra/0.1.0/features
```

The second response should show the crate's feature/build information; inspect it rather than treating a response alone as proof of the requested feature build.

## 7. Install and smoke-test from crates.io

```sh
cargo install zarra-cli
zarra --help
```

Confirm that `convert`, `info`, `query`, `sql`, `tile`, and `serve` are listed, then run a real bounded SQL smoke against the committed fixture:

```sh
zarra sql testdata/refpy.zarr \
  "SELECT band, COUNT(value) AS valid FROM raster GROUP BY band"
```

This registry installation is distinct from the earlier `cargo install --path crates/zarra-cli` check.
