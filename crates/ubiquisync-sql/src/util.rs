/// `quote_ident("name")` → `"name"`, `quote_ident("select")` → `"select"`
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
