//! Small SQL string helpers shared across backends.

/// Quote a SQL identifier by wrapping it in double quotes and doubling any
/// internal quote, so names that collide with keywords or contain specials are
/// safe to splice into SQL. `quote_ident("select")` → `"select"`; a name
/// containing a quote has it doubled: `quote_ident("a\"b")` → `"a""b"`.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quote_ident_wraps_in_double_quotes() {
        assert_eq!(quote_ident("name"), "\"name\"");
    }
    #[test]
    fn quote_ident_escapes_internal_double_quotes() {
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
    }
}
