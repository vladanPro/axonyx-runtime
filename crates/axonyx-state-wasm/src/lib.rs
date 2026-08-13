#![deny(unsafe_op_in_unsafe_fn)]

pub const AX_STATE_ABI_VERSION: u32 = 1;
pub const AX_STATE_OP_SET: u32 = 0;
pub const AX_STATE_OP_ADD: u32 = 1;
pub const AX_STATE_OP_SUB: u32 = 2;
pub const AX_STATE_OP_TOGGLE: u32 = 3;
pub const AX_STATE_UNSUPPORTED_BOOL: u32 = u32::MAX;

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_abi_version() -> u32 {
    AX_STATE_ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_apply_number(operation: u32, current: f64, operand: f64) -> f64 {
    match operation {
        AX_STATE_OP_SET => operand,
        AX_STATE_OP_ADD => current + operand,
        AX_STATE_OP_SUB => current - operand,
        _ => f64::NAN,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_apply_bool(operation: u32, current: u32, operand: u32) -> u32 {
    match operation {
        AX_STATE_OP_SET => u32::from(operand != 0),
        AX_STATE_OP_TOGGLE => u32::from(current == 0),
        _ => AX_STATE_UNSUPPORTED_BOOL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_stable_v0_abi() {
        assert_eq!(ax_state_abi_version(), 1);
    }

    #[test]
    fn applies_number_operations() {
        assert_eq!(ax_state_apply_number(AX_STATE_OP_SET, 2.0, 7.0), 7.0);
        assert_eq!(ax_state_apply_number(AX_STATE_OP_ADD, 2.0, 3.0), 5.0);
        assert_eq!(ax_state_apply_number(AX_STATE_OP_SUB, 2.0, 3.0), -1.0);
        assert!(ax_state_apply_number(AX_STATE_OP_TOGGLE, 0.0, 0.0).is_nan());
    }

    #[test]
    fn applies_bool_operations() {
        assert_eq!(ax_state_apply_bool(AX_STATE_OP_SET, 0, 1), 1);
        assert_eq!(ax_state_apply_bool(AX_STATE_OP_SET, 1, 0), 0);
        assert_eq!(ax_state_apply_bool(AX_STATE_OP_TOGGLE, 0, 0), 1);
        assert_eq!(ax_state_apply_bool(AX_STATE_OP_TOGGLE, 1, 0), 0);
        assert_eq!(
            ax_state_apply_bool(AX_STATE_OP_ADD, 0, 0),
            AX_STATE_UNSUPPORTED_BOOL
        );
    }
}
