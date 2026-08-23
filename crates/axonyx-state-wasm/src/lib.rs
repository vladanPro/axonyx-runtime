#![deny(unsafe_op_in_unsafe_fn)]

pub const AX_STATE_ABI_VERSION: u32 = 3;
pub const AX_STATE_OP_SET: u32 = 0;
pub const AX_STATE_OP_ADD: u32 = 1;
pub const AX_STATE_OP_SUB: u32 = 2;
pub const AX_STATE_OP_TOGGLE: u32 = 3;
pub const AX_STATE_TYPE_STRING: u32 = 0;
pub const AX_STATE_TYPE_NUMBER: u32 = 1;
pub const AX_STATE_TYPE_BOOL: u32 = 2;
pub const AX_STATE_TYPE_VALUE: u32 = 3;
pub const AX_STATE_UNSUPPORTED_BOOL: u32 = u32::MAX;
pub const AX_STATE_STRING_ERROR: u32 = u32::MAX;
pub const AX_STATE_STRING_CAPACITY: usize = 4096;
pub const AX_STATE_VALUE_CAPACITY: usize = 64 * 1024;
pub const AX_STATE_VALUE_FRAME_VERSION: u8 = 1;
pub const AX_STATE_VALUE_MAX_DEPTH: usize = 32;

static mut STRING_BUFFER: [u8; AX_STATE_STRING_CAPACITY] = [0; AX_STATE_STRING_CAPACITY];
static mut VALUE_BUFFER: [u8; AX_STATE_VALUE_CAPACITY] = [0; AX_STATE_VALUE_CAPACITY];

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_abi_version() -> u32 {
    AX_STATE_ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_supports_operation(value_type: u32, operation: u32) -> u32 {
    u32::from(matches!(
        (value_type, operation),
        (AX_STATE_TYPE_STRING, AX_STATE_OP_SET)
            | (
                AX_STATE_TYPE_NUMBER,
                AX_STATE_OP_SET | AX_STATE_OP_ADD | AX_STATE_OP_SUB
            )
            | (AX_STATE_TYPE_BOOL, AX_STATE_OP_SET | AX_STATE_OP_TOGGLE)
            | (AX_STATE_TYPE_VALUE, AX_STATE_OP_SET)
    ))
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

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_string_buffer_ptr() -> *mut u8 {
    std::ptr::addr_of_mut!(STRING_BUFFER).cast::<u8>()
}

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_string_buffer_capacity() -> u32 {
    AX_STATE_STRING_CAPACITY as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_apply_string(operation: u32, operand_len: u32) -> u32 {
    let Ok(operand_len) = usize::try_from(operand_len) else {
        return AX_STATE_STRING_ERROR;
    };
    if operation != AX_STATE_OP_SET || operand_len > AX_STATE_STRING_CAPACITY {
        return AX_STATE_STRING_ERROR;
    }

    // The browser host writes into this exported scratch buffer before calling us.
    let bytes = unsafe { std::slice::from_raw_parts(ax_state_string_buffer_ptr(), operand_len) };
    if std::str::from_utf8(bytes).is_err() {
        return AX_STATE_STRING_ERROR;
    }
    operand_len as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_value_buffer_ptr() -> *mut u8 {
    std::ptr::addr_of_mut!(VALUE_BUFFER).cast::<u8>()
}

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_value_buffer_capacity() -> u32 {
    AX_STATE_VALUE_CAPACITY as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_apply_value(operation: u32, operand_len: u32) -> u32 {
    let Ok(operand_len) = usize::try_from(operand_len) else {
        return AX_STATE_STRING_ERROR;
    };
    if operation != AX_STATE_OP_SET || operand_len > AX_STATE_VALUE_CAPACITY {
        return AX_STATE_STRING_ERROR;
    }

    let bytes = unsafe { std::slice::from_raw_parts(ax_state_value_buffer_ptr(), operand_len) };
    match validate_value_frame(bytes, 0) {
        Some(consumed) if consumed == bytes.len() => operand_len as u32,
        _ => AX_STATE_STRING_ERROR,
    }
}

fn validate_value_frame(bytes: &[u8], depth: usize) -> Option<usize> {
    if depth > AX_STATE_VALUE_MAX_DEPTH
        || bytes.len() < 4
        || bytes[0] != b'A'
        || bytes[1] != b'X'
        || bytes[2] != AX_STATE_VALUE_FRAME_VERSION
    {
        return None;
    }

    let mut cursor = 4usize;
    match bytes[3] {
        0 => {}
        1 => {
            let length = read_u32(bytes, &mut cursor)? as usize;
            let value = take(bytes, &mut cursor, length)?;
            std::str::from_utf8(value).ok()?;
        }
        2 => {
            let value = *take(bytes, &mut cursor, 1)?.first()?;
            if value > 1 {
                return None;
            }
        }
        3 => {
            let raw: [u8; 8] = take(bytes, &mut cursor, 8)?.try_into().ok()?;
            if !f64::from_le_bytes(raw).is_finite() {
                return None;
            }
        }
        4 => {
            take(bytes, &mut cursor, 8)?;
        }
        5 => {
            let length = read_u32(bytes, &mut cursor)? as usize;
            take(bytes, &mut cursor, length)?;
        }
        6 => {
            let count = read_u32(bytes, &mut cursor)? as usize;
            for _ in 0..count {
                let consumed = validate_value_frame(&bytes[cursor..], depth + 1)?;
                cursor = cursor.checked_add(consumed)?;
            }
        }
        7 => {
            let count = read_u32(bytes, &mut cursor)? as usize;
            for _ in 0..count {
                let key_length = read_u32(bytes, &mut cursor)? as usize;
                let key = take(bytes, &mut cursor, key_length)?;
                std::str::from_utf8(key).ok()?;
                let consumed = validate_value_frame(&bytes[cursor..], depth + 1)?;
                cursor = cursor.checked_add(consumed)?;
            }
        }
        _ => return None,
    }
    Some(cursor)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let raw: [u8; 4] = take(bytes, cursor, 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, length: usize) -> Option<&'a [u8]> {
    let end = cursor.checked_add(length)?;
    let value = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_stable_v2_abi() {
        assert_eq!(ax_state_abi_version(), 3);
    }

    #[test]
    fn reports_supported_type_operation_pairs() {
        assert_eq!(
            ax_state_supports_operation(AX_STATE_TYPE_STRING, AX_STATE_OP_SET),
            1
        );
        assert_eq!(
            ax_state_supports_operation(AX_STATE_TYPE_STRING, AX_STATE_OP_ADD),
            0
        );
        assert_eq!(
            ax_state_supports_operation(AX_STATE_TYPE_NUMBER, AX_STATE_OP_SUB),
            1
        );
        assert_eq!(
            ax_state_supports_operation(AX_STATE_TYPE_BOOL, AX_STATE_OP_TOGGLE),
            1
        );
        assert_eq!(
            ax_state_supports_operation(AX_STATE_TYPE_VALUE, AX_STATE_OP_SET),
            1
        );
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

    #[test]
    fn validates_utf8_string_set_payloads() {
        let value = "Axonyx zdravo".as_bytes();
        unsafe {
            std::ptr::copy_nonoverlapping(
                value.as_ptr(),
                ax_state_string_buffer_ptr(),
                value.len(),
            );
        }
        assert_eq!(
            ax_state_apply_string(AX_STATE_OP_SET, value.len() as u32),
            value.len() as u32
        );
        assert_eq!(
            ax_state_apply_string(AX_STATE_OP_ADD, value.len() as u32),
            AX_STATE_STRING_ERROR
        );
        assert_eq!(
            ax_state_apply_string(AX_STATE_OP_SET, AX_STATE_STRING_CAPACITY as u32 + 1),
            AX_STATE_STRING_ERROR
        );
    }

    #[test]
    fn validates_nested_binary_value_frames() {
        let string = value_frame(1, &[2, 0, 0, 0, b'o', b'k']);
        let number = value_frame(3, &2.5f64.to_le_bytes());
        let mut list_payload = vec![2, 0, 0, 0];
        list_payload.extend_from_slice(&string);
        list_payload.extend_from_slice(&number);
        let list = value_frame(6, &list_payload);

        unsafe {
            std::ptr::copy_nonoverlapping(list.as_ptr(), ax_state_value_buffer_ptr(), list.len());
        }
        assert_eq!(
            ax_state_apply_value(AX_STATE_OP_SET, list.len() as u32),
            list.len() as u32
        );

        let mut invalid = list;
        invalid[0] = b'J';
        unsafe {
            std::ptr::copy_nonoverlapping(
                invalid.as_ptr(),
                ax_state_value_buffer_ptr(),
                invalid.len(),
            );
        }
        assert_eq!(
            ax_state_apply_value(AX_STATE_OP_SET, invalid.len() as u32),
            AX_STATE_STRING_ERROR
        );
    }

    fn value_frame(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![b'A', b'X', AX_STATE_VALUE_FRAME_VERSION, tag];
        frame.extend_from_slice(payload);
        frame
    }
}
