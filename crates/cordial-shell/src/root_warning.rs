//! What running as root costs, said once before it costs it.
//!
//! A FreeBSD user reached the Roblox home page, signed in, entered an
//! experience, and then the engine called `abort()`. The cause was three lines
//! above it in their log: no PipeWire session, so `opensles.cpp` reported
//! failure honestly, and the engine's answer to an honest audio failure is to
//! stop. They were running as root, which is why there was no session bus to
//! find a PipeWire daemon through.
//!
//! Root is not itself the fault, and this does not refuse it — on FreeBSD's
//! linuxulator there is no other user, so refusing would refuse the platform.
//! What it does is name the consequences before an hour is spent on them,
//! because each one presents as a different unrelated bug:
//!
//! - **No session bus**, so no keyring. Cookies fall back to a 0600 file, which
//!   `secrets.rs` already warns about, and anything able to read that file has
//!   the account.
//! - **No PipeWire**, so no audio — and the engine aborts at experience start
//!   rather than playing silence. That is the one that looks like a crash.
//! - No Feral GameMode, no accessibility bus.
//!
//! The launcher says what will happen, on stderr, and carries on, because
//! sometimes root is the only option available and the job is to be honest
//! rather than obstructive. `stacked` prints it on every launch as root;
//! nothing is remembered between launches, because a warning about losing your
//! account to a readable file is not one to make dismissible forever on a
//! machine where it stays true.

/// Whether this process is running as root.
pub fn running_as_root() -> bool {
    // SAFETY: `geteuid` takes no arguments, touches no memory and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// What running as root will cost, for the launcher to print before it
/// starts the client.
pub const WARNING: &str = "\
Running as root. Roblox will have no sound, and the engine stops when an
experience starts if it cannot open an audio device. Your sign-in is also saved
to a plain file instead of the keyring, so anything that can read your files
can take the account. This happens because root usually has no desktop session
for PipeWire and the keyring to live in; run Stacked as an ordinary user if you
can.";
