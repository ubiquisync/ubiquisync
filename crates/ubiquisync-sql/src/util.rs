/// `quote_ident("name")` → `"name"`, `quote_ident("select")` → `"select"`
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
