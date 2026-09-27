//! Registration predicates; these do not prove ownership or normalize input.

pub fn email(value: &str) -> bool {
    if !value.is_ascii() || value.len() > 254 {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&b))
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Baseline for password-only registration. Preserve Unicode and whitespace.
pub fn password(value: &str) -> bool {
    value.len() <= crate::password::MAX_PASSWORD_BYTES && value.chars().count() >= 15
}

#[cfg(test)]
mod tests {
    #[test]
    fn registration_policy_is_bounded_and_does_not_trim() {
        assert!(super::email("first+tag@example.com"));
        for value in [
            "",
            "a@",
            "a@b",
            "a..b@example.com",
            "a@example.com@evil.com",
            " a@example.com",
            "a@-example.com",
        ] {
            assert!(!super::email(value), "{value}");
        }
        assert!(!super::password("short"));
        assert!(super::password("long pass phrase here"));
        assert!(super::password(&"界".repeat(15)));
        assert!(!super::password(&"a".repeat(1025)));
    }
}
