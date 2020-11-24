#!/bin/sh

# We try to support some older versions of rustc. However, the support is
# tiered a bit. Our dev-dependencies do *not* guarantee that old minimal
# version. So we don't do tests on the older ones. Also, the
# signal-hook-registry supports older rustc than we signal-hook.

set -ex

rm -f Cargo.lock
cargo build --all --exclude signal-hook-async-std

if [ "$RUST_VERSION" = 1.36.0 ] ; then
	exit
fi

if [ "$OS" = "windows-latest" ] ; then
	# The async support crates rely on the iterator module
	# which isn't available for windows. So exclude them
	# from the build.
	EXCLUDE_FROM_BUILD="--exclude signal-hook-mio --exclude signal-hook-tokio --exclude signal-hook-async-std"
else
	EXCLUDE_FROM_BUILD=""
fi

export RUSTFLAGS="-D warnings"

cargo test --all --all-features $EXCLUDE_FROM_BUILD
cargo test --all $EXCLUDE_FROM_BUILD
