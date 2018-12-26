# 0.1.7

* The `Signals` iterator allows adding signals after creation.
* Fixed a bug where `Signals` registrations could be unregirestered too soon if
  the `Signals` was cloned previously.

# 0.1.6

* The internally used ArcSwap thing doesn't block other ArcSwaps now (has
  independent generation lock).

# 0.1.5

* Re-exported signal constants, so users no longer need libc.

# 0.1.4

* Compilation fix for android-aarch64

# 0.1.3

* Tokio support.
* Mio support.
* Dependency updates.

# 0.1.2

* Dependency updates.

# 0.1.1

* Get rid of `catch_unwind` inside the signal handler.
* Link to the nix crate.

# 0.1.0

* Initial basic implementation.
* Flag helpers.
* Pipe helpers.
* High-level iterator helper.
