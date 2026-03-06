#[allow(dead_code)]
pub const HOLD_DOUBLE_TAP_WINDOW_MS: u64 = crate::interaction_sm::DEFAULT_DOUBLE_TAP_WINDOW_MS;
pub use crate::interaction_sm::InteractionEffect as HotkeyEffect;
pub use crate::interaction_sm::InteractionInput as HotkeyInput;
pub use crate::interaction_sm::InteractionState as HotkeyState;
pub use crate::interaction_sm::RuntimeSnapshot;
