//! Device-level keyboard capture for Azad.
//!
//! A root helper seizes keyboards through IOKit, runs Azad's shortcut [`policy`] on normalized
//! report batches ([`engine`]), sends claimed actions to the authenticated Azad app, and forwards
//! everything else through the Karabiner DriverKit virtual keyboard ([`vhid`]). Capturing below
//! application event taps keeps shortcuts working while Secure Input is on or another process
//! holds a consuming event tap.

pub mod engine;
pub mod hid;
pub mod ipc;
pub mod policy;
pub mod sys;
pub mod vhid;
