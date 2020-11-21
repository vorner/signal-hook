#!/bin/sh

# We try to support some older versions of rustc. However, the support is
# tiered a bit. Our dev-dependencies do *not* guarantee that old minimal
# version. So we don't do tests on the older ones. Also, the
# signal-hook-registry supports older rustc than we signal-hook.

set -ex

if [ "$RUST_VERSION" = 1.26.0 ] ; then
	rm Cargo.toml
	cd signal-hook-registry
	sed -i -e '/signal-hook =/d' Cargo.toml
	cargo check
	exit
fi

rm -f Cargo.lock
cargo build --all

if [ "$RUST_VERSION" = 1.31.0 ] ; then
	exit
fi

if [ "$OS" = "windows-latest" ] ; then
	# The async support crates rely on the iterator module
	# which isn't available for windows. So exclude them
	# from the build.
	EXCLUDE_FROM_BUILD="--exclude signal-hook-mio --exclude signal-hook-tokio"
else
	EXCLUDE_FROM_BUILD=""
fi

export RUSTFLAGS="-D warnings"

cargo build --all --all-features $EXCLUDE_FROM_BUILD
cargo test --all --all-features $EXCLUDE_FROM_BUILD
cargo test --all $EXCLUDE_FROM_BUILD
cargo doc --no-deps

# Sometimes nightly doesn't have clippy or rustfmt, so don't try that there.
if [ "$RUST_VERSION" = nightly ] ; then
	exit
fi

cargo clippy --all $EXCLUDE_FROM_BUILD --tests -- --deny clippy::all
