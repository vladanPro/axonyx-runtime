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
pub const AX_EXPRESSION_PROGRAM_VERSION: u8 = 1;

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

#[derive(Debug, Clone, PartialEq)]
enum ExprValue {
    Null,
    String(String),
    Bool(bool),
    Float(f64),
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<ExprValue>),
    Object(Vec<(String, ExprValue)>),
}

#[unsafe(no_mangle)]
pub extern "C" fn ax_state_evaluate_expression(request_len: u32) -> u32 {
    let Ok(request_len) = usize::try_from(request_len) else {
        return AX_STATE_STRING_ERROR;
    };
    if request_len > AX_STATE_VALUE_CAPACITY {
        return AX_STATE_STRING_ERROR;
    }

    let request = unsafe { std::slice::from_raw_parts(ax_state_value_buffer_ptr(), request_len) };
    let Some(result) = evaluate_expression_request(request) else {
        return AX_STATE_STRING_ERROR;
    };
    let mut encoded = Vec::new();
    if encode_expr_value(&result, &mut encoded, 0).is_none()
        || encoded.len() > AX_STATE_VALUE_CAPACITY
    {
        return AX_STATE_STRING_ERROR;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(encoded.as_ptr(), ax_state_value_buffer_ptr(), encoded.len());
    }
    encoded.len() as u32
}

fn evaluate_expression_request(request: &[u8]) -> Option<ExprValue> {
    let mut cursor = 0usize;
    let program_len = read_u32(request, &mut cursor)? as usize;
    let program = take(request, &mut cursor, program_len)?;
    let dependency_count = read_u32(request, &mut cursor)? as usize;
    if dependency_count > request.len().saturating_sub(cursor) / 4 {
        return None;
    }
    let mut dependencies = Vec::with_capacity(dependency_count);
    for _ in 0..dependency_count {
        let (value, consumed) = decode_expr_value(&request[cursor..], 0)?;
        cursor = cursor.checked_add(consumed)?;
        dependencies.push(value);
    }
    if cursor != request.len() {
        return None;
    }
    evaluate_expression_program(program, &dependencies)
}

fn evaluate_expression_program(program: &[u8], dependencies: &[ExprValue]) -> Option<ExprValue> {
    if program.len() < 4 || &program[..4] != b"AXE\x01" {
        return None;
    }
    let mut cursor = 4usize;
    let mut stack = Vec::new();
    while cursor < program.len() {
        let opcode = *program.get(cursor)?;
        cursor += 1;
        match opcode {
            0 => {
                let raw: [u8; 2] = take(program, &mut cursor, 2)?.try_into().ok()?;
                stack.push(dependencies.get(u16::from_le_bytes(raw) as usize)?.clone());
            }
            1 => stack.push(ExprValue::Null),
            2 => stack.push(ExprValue::String(read_program_string(
                program,
                &mut cursor,
            )?)),
            3 => {
                let value = *take(program, &mut cursor, 1)?.first()?;
                if value > 1 {
                    return None;
                }
                stack.push(ExprValue::Bool(value == 1));
            }
            4 => {
                let raw: [u8; 8] = take(program, &mut cursor, 8)?.try_into().ok()?;
                let value = f64::from_le_bytes(raw);
                if !value.is_finite() {
                    return None;
                }
                stack.push(ExprValue::Float(value));
            }
            5 => {
                let raw: [u8; 8] = take(program, &mut cursor, 8)?.try_into().ok()?;
                stack.push(ExprValue::Int(i64::from_le_bytes(raw)));
            }
            10 => {
                let value = stack.pop()?;
                stack.push(ExprValue::Bool(!expr_truthy(&value)));
            }
            11 => {
                let value = stack.pop()?;
                stack.push(match value {
                    ExprValue::Int(value) => ExprValue::Int(value.checked_neg()?),
                    ExprValue::Float(value) => finite_float(-value)?,
                    _ => return None,
                });
            }
            20..=34 | 40 => {
                let right = stack.pop()?;
                let left = stack.pop()?;
                stack.push(evaluate_binary_opcode(opcode, left, right)?);
            }
            41 | 42 => {
                let property = read_program_string(program, &mut cursor)?;
                let object = stack.pop()?;
                let optional = opcode == 42;
                stack.push(match object {
                    ExprValue::Object(fields) => fields
                        .into_iter()
                        .find_map(|(name, value)| (name == property).then_some(value))
                        .unwrap_or(ExprValue::Null),
                    ExprValue::Null if optional => ExprValue::Null,
                    _ if optional => ExprValue::Null,
                    _ => return None,
                });
            }
            _ => return None,
        }
    }
    (stack.len() == 1).then(|| stack.pop()).flatten()
}

fn evaluate_binary_opcode(opcode: u8, left: ExprValue, right: ExprValue) -> Option<ExprValue> {
    match opcode {
        20 => expr_add(left, right),
        21 => expr_numeric(
            left,
            right,
            i64::checked_sub,
            |left, right| left - right,
            false,
        ),
        22 => expr_numeric(
            left,
            right,
            i64::checked_mul,
            |left, right| left * right,
            false,
        ),
        23 => expr_numeric(
            left,
            right,
            i64::checked_div,
            |left, right| left / right,
            true,
        ),
        24 => expr_numeric(
            left,
            right,
            i64::checked_rem,
            |left, right| left % right,
            true,
        ),
        25 => Some(ExprValue::Bool(expr_values_equal(&left, &right))),
        26 => Some(ExprValue::Bool(!expr_values_equal(&left, &right))),
        27 => Some(ExprValue::Bool(expr_compare(&left, &right)?.is_gt())),
        28 => Some(ExprValue::Bool(expr_compare(&left, &right)?.is_ge())),
        29 => Some(ExprValue::Bool(expr_compare(&left, &right)?.is_lt())),
        30 => Some(ExprValue::Bool(expr_compare(&left, &right)?.is_le())),
        31 => match right {
            ExprValue::List(items) => Some(ExprValue::Bool(
                items.iter().any(|item| expr_values_equal(&left, item)),
            )),
            _ => None,
        },
        32 => Some(ExprValue::Bool(expr_truthy(&left) && expr_truthy(&right))),
        33 => Some(ExprValue::Bool(expr_truthy(&left) || expr_truthy(&right))),
        34 => Some(if matches!(left, ExprValue::Null) {
            right
        } else {
            left
        }),
        40 => expr_index(left, right),
        _ => None,
    }
}

fn expr_add(left: ExprValue, right: ExprValue) -> Option<ExprValue> {
    match (left, right) {
        (ExprValue::Int(left), ExprValue::Int(right)) => {
            Some(ExprValue::Int(left.checked_add(right)?))
        }
        (ExprValue::Float(left), ExprValue::Float(right)) => finite_float(left + right),
        (ExprValue::Int(left), ExprValue::Float(right)) => finite_float(left as f64 + right),
        (ExprValue::Float(left), ExprValue::Int(right)) => finite_float(left + right as f64),
        (left, right) => Some(ExprValue::String(format!(
            "{}{}",
            expr_string(&left),
            expr_string(&right)
        ))),
    }
}

fn expr_numeric(
    left: ExprValue,
    right: ExprValue,
    int_op: impl FnOnce(i64, i64) -> Option<i64>,
    float_op: impl FnOnce(f64, f64) -> f64,
    reject_zero: bool,
) -> Option<ExprValue> {
    let right_is_zero = match &right {
        ExprValue::Int(value) => *value == 0,
        ExprValue::Float(value) => *value == 0.0,
        _ => false,
    };
    if reject_zero && right_is_zero {
        return None;
    }
    match (left, right) {
        (ExprValue::Int(left), ExprValue::Int(right)) => Some(ExprValue::Int(int_op(left, right)?)),
        (ExprValue::Float(left), ExprValue::Float(right)) => finite_float(float_op(left, right)),
        (ExprValue::Int(left), ExprValue::Float(right)) => {
            finite_float(float_op(left as f64, right))
        }
        (ExprValue::Float(left), ExprValue::Int(right)) => {
            finite_float(float_op(left, right as f64))
        }
        _ => None,
    }
}

fn finite_float(value: f64) -> Option<ExprValue> {
    value.is_finite().then_some(ExprValue::Float(value))
}

fn expr_values_equal(left: &ExprValue, right: &ExprValue) -> bool {
    match (left, right) {
        (ExprValue::Int(left), ExprValue::Float(right)) => *left as f64 == *right,
        (ExprValue::Float(left), ExprValue::Int(right)) => *left == *right as f64,
        _ => left == right,
    }
}

fn expr_compare(left: &ExprValue, right: &ExprValue) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (ExprValue::Int(left), ExprValue::Int(right)) => Some(left.cmp(right)),
        (ExprValue::Float(left), ExprValue::Float(right)) => left.partial_cmp(right),
        (ExprValue::Int(left), ExprValue::Float(right)) => (*left as f64).partial_cmp(right),
        (ExprValue::Float(left), ExprValue::Int(right)) => left.partial_cmp(&(*right as f64)),
        (ExprValue::String(left), ExprValue::String(right)) => Some(left.cmp(right)),
        _ => None,
    }
}

fn expr_index(object: ExprValue, index: ExprValue) -> Option<ExprValue> {
    match (object, index) {
        (ExprValue::List(items), ExprValue::Int(index)) => usize::try_from(index)
            .ok()
            .and_then(|index| items.get(index).cloned())
            .or(Some(ExprValue::Null)),
        (ExprValue::Object(fields), ExprValue::String(key)) => fields
            .into_iter()
            .find_map(|(name, value)| (name == key).then_some(value))
            .or(Some(ExprValue::Null)),
        _ => None,
    }
}

fn expr_truthy(value: &ExprValue) -> bool {
    match value {
        ExprValue::Null => false,
        ExprValue::String(value) => !value.is_empty(),
        ExprValue::Bool(value) => *value,
        ExprValue::Float(value) => *value != 0.0,
        ExprValue::Int(value) => *value != 0,
        ExprValue::Bytes(value) => !value.is_empty(),
        ExprValue::List(value) => !value.is_empty(),
        ExprValue::Object(value) => !value.is_empty(),
    }
}

fn expr_string(value: &ExprValue) -> String {
    match value {
        ExprValue::Null => String::new(),
        ExprValue::String(value) => value.clone(),
        ExprValue::Bool(value) => value.to_string(),
        ExprValue::Float(value) => value.to_string(),
        ExprValue::Int(value) => value.to_string(),
        ExprValue::Bytes(_) | ExprValue::List(_) | ExprValue::Object(_) => String::new(),
    }
}

fn read_program_string(program: &[u8], cursor: &mut usize) -> Option<String> {
    let length = read_u32(program, cursor)? as usize;
    String::from_utf8(take(program, cursor, length)?.to_vec()).ok()
}

fn decode_expr_value(bytes: &[u8], depth: usize) -> Option<(ExprValue, usize)> {
    if depth > AX_STATE_VALUE_MAX_DEPTH
        || bytes.len() < 4
        || bytes[0] != b'A'
        || bytes[1] != b'X'
        || bytes[2] != AX_STATE_VALUE_FRAME_VERSION
    {
        return None;
    }
    let mut cursor = 4usize;
    let value = match bytes[3] {
        0 => ExprValue::Null,
        1 => {
            let length = read_u32(bytes, &mut cursor)? as usize;
            ExprValue::String(String::from_utf8(take(bytes, &mut cursor, length)?.to_vec()).ok()?)
        }
        2 => {
            let value = *take(bytes, &mut cursor, 1)?.first()?;
            if value > 1 {
                return None;
            }
            ExprValue::Bool(value == 1)
        }
        3 => {
            let raw: [u8; 8] = take(bytes, &mut cursor, 8)?.try_into().ok()?;
            let value = f64::from_le_bytes(raw);
            if !value.is_finite() {
                return None;
            }
            ExprValue::Float(value)
        }
        4 => {
            let raw: [u8; 8] = take(bytes, &mut cursor, 8)?.try_into().ok()?;
            ExprValue::Int(i64::from_le_bytes(raw))
        }
        5 => {
            let length = read_u32(bytes, &mut cursor)? as usize;
            ExprValue::Bytes(take(bytes, &mut cursor, length)?.to_vec())
        }
        6 => {
            let count = read_u32(bytes, &mut cursor)? as usize;
            if count > bytes.len().saturating_sub(cursor) / 4 {
                return None;
            }
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                let (item, consumed) = decode_expr_value(&bytes[cursor..], depth + 1)?;
                cursor = cursor.checked_add(consumed)?;
                items.push(item);
            }
            ExprValue::List(items)
        }
        7 => {
            let count = read_u32(bytes, &mut cursor)? as usize;
            if count > bytes.len().saturating_sub(cursor) / 8 {
                return None;
            }
            let mut fields = Vec::with_capacity(count);
            for _ in 0..count {
                let key_length = read_u32(bytes, &mut cursor)? as usize;
                let key = String::from_utf8(take(bytes, &mut cursor, key_length)?.to_vec()).ok()?;
                if fields.iter().any(|(existing, _)| existing == &key) {
                    return None;
                }
                let (value, consumed) = decode_expr_value(&bytes[cursor..], depth + 1)?;
                cursor = cursor.checked_add(consumed)?;
                fields.push((key, value));
            }
            ExprValue::Object(fields)
        }
        _ => return None,
    };
    Some((value, cursor))
}

fn encode_expr_value(value: &ExprValue, output: &mut Vec<u8>, depth: usize) -> Option<()> {
    if depth > AX_STATE_VALUE_MAX_DEPTH {
        return None;
    }
    output.extend_from_slice(b"AX");
    output.push(AX_STATE_VALUE_FRAME_VERSION);
    match value {
        ExprValue::Null => output.push(0),
        ExprValue::String(value) => {
            output.push(1);
            push_u32(output, value.len())?;
            output.extend_from_slice(value.as_bytes());
        }
        ExprValue::Bool(value) => {
            output.push(2);
            output.push(u8::from(*value));
        }
        ExprValue::Float(value) => {
            if !value.is_finite() {
                return None;
            }
            output.push(3);
            output.extend_from_slice(&value.to_le_bytes());
        }
        ExprValue::Int(value) => {
            output.push(4);
            output.extend_from_slice(&value.to_le_bytes());
        }
        ExprValue::Bytes(value) => {
            output.push(5);
            push_u32(output, value.len())?;
            output.extend_from_slice(value);
        }
        ExprValue::List(items) => {
            output.push(6);
            push_u32(output, items.len())?;
            for item in items {
                encode_expr_value(item, output, depth + 1)?;
            }
        }
        ExprValue::Object(fields) => {
            output.push(7);
            push_u32(output, fields.len())?;
            for (key, value) in fields {
                push_u32(output, key.len())?;
                output.extend_from_slice(key.as_bytes());
                encode_expr_value(value, output, depth + 1)?;
            }
        }
    }
    Some(())
}

fn push_u32(output: &mut Vec<u8>, value: usize) -> Option<()> {
    output.extend_from_slice(&u32::try_from(value).ok()?.to_le_bytes());
    Some(())
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
    use std::sync::Mutex;

    static BUFFER_TEST_LOCK: Mutex<()> = Mutex::new(());

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
        let _guard = BUFFER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
        let _guard = BUFFER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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

    #[test]
    fn evaluates_compiler_expression_bytecode_with_multiple_dependencies() {
        let _guard = BUFFER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut program = b"AXE\x01".to_vec();
        program.extend_from_slice(&[0, 0, 0]);
        program.extend_from_slice(&[0, 1, 0]);
        program.push(20);
        program.push(5);
        program.extend_from_slice(&2i64.to_le_bytes());
        program.push(22);

        let mut request = Vec::new();
        request.extend_from_slice(&(program.len() as u32).to_le_bytes());
        request.extend_from_slice(&program);
        request.extend_from_slice(&2u32.to_le_bytes());
        request.extend_from_slice(&value_frame(4, &3i64.to_le_bytes()));
        request.extend_from_slice(&value_frame(4, &4i64.to_le_bytes()));
        unsafe {
            std::ptr::copy_nonoverlapping(
                request.as_ptr(),
                ax_state_value_buffer_ptr(),
                request.len(),
            );
        }

        let result_len = ax_state_evaluate_expression(request.len() as u32);
        assert_ne!(result_len, AX_STATE_STRING_ERROR);
        let result =
            unsafe { std::slice::from_raw_parts(ax_state_value_buffer_ptr(), result_len as usize) };
        assert_eq!(decode_expr_value(result, 0), Some((ExprValue::Int(14), 12)));
    }

    #[test]
    fn evaluates_object_member_and_boolean_comparison() {
        let _guard = BUFFER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut program = b"AXE\x01".to_vec();
        program.extend_from_slice(&[0, 0, 0]);
        program.push(41);
        program.extend_from_slice(&6u32.to_le_bytes());
        program.extend_from_slice(b"active");
        program.push(3);
        program.push(1);
        program.push(25);

        let mut object_payload = 1u32.to_le_bytes().to_vec();
        object_payload.extend_from_slice(&6u32.to_le_bytes());
        object_payload.extend_from_slice(b"active");
        object_payload.extend_from_slice(&value_frame(2, &[1]));
        let mut request = Vec::new();
        request.extend_from_slice(&(program.len() as u32).to_le_bytes());
        request.extend_from_slice(&program);
        request.extend_from_slice(&1u32.to_le_bytes());
        request.extend_from_slice(&value_frame(7, &object_payload));
        unsafe {
            std::ptr::copy_nonoverlapping(
                request.as_ptr(),
                ax_state_value_buffer_ptr(),
                request.len(),
            );
        }

        let result_len = ax_state_evaluate_expression(request.len() as u32);
        assert_ne!(result_len, AX_STATE_STRING_ERROR);
        let result =
            unsafe { std::slice::from_raw_parts(ax_state_value_buffer_ptr(), result_len as usize) };
        assert_eq!(
            decode_expr_value(result, 0),
            Some((ExprValue::Bool(true), 5))
        );
    }

    #[test]
    fn rejects_impossible_expression_collection_counts_before_allocation() {
        let mut request = Vec::new();
        request.extend_from_slice(&4u32.to_le_bytes());
        request.extend_from_slice(b"AXE\x01");
        request.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(evaluate_expression_request(&request), None);

        let mut list = value_frame(6, &u32::MAX.to_le_bytes());
        assert_eq!(decode_expr_value(&list, 0), None);
        list[3] = 7;
        assert_eq!(decode_expr_value(&list, 0), None);
    }

    fn value_frame(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![b'A', b'X', AX_STATE_VALUE_FRAME_VERSION, tag];
        frame.extend_from_slice(payload);
        frame
    }
}
