#![allow(deprecated)]
#![allow(unexpected_cfgs)]
#![allow(unsafe_op_in_unsafe_fn)]

mod app;
mod config;
mod device;
mod hotkey_sm;
mod platform;
mod preferred_store;
mod speech;

fn main() {
    app::run();
}
