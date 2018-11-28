# Upcoming (unreleased) changes

* Exposed a new `SigNoInt` type to represent the correct integer type for
  signal numbers.  This prevents users of this library from having to
  directly address `libc::c_int` or know that they are `i32`s.

* Exposed signal numbers as a `SigNo` enum with the ability to format their
  values as user-friendly strings.  Previous implementations exposed these
  as raw `i32`s but the internal representation of `SigNo` is the same so
  old code should continue to compile unmodified.

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
