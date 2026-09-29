//! Device-level keyboard capture for Azad.
//!
//! A root helper seizes keyboards through IOKit, runs Azad's shortcut [`policy`] on normalized
//! report batches ([`engine`]), sends claimed actions to the authenticated Azad app, and forwards
//! everything else through the Karabiner DriverKit virtual keyboard ([`vhid`]). Capturing below
//! application event taps keeps shortcuts working while Secure Input is on or another process
//! holds a consuming event tap.
//!
//! The policy, report engine, wire protocol and driver client are plain Rust; the IOKit device
//! manager, peer authentication and platform bindings used by the helper binary are behind the
//! `helper` feature.

pub mod clock;
pub mod engine;
#[cfg(feature = "helper")]
pub mod hid;
#[cfg(feature = "helper")]
pub mod ipc;
pub mod policy;
pub mod protocol;
#[cfg(feature = "helper")]
pub mod sys;
pub mod vhid;
