/// Expands `%NAME%` references the way cmd.exe does. Unknown names are left as they
/// are so the resulting path is visibly wrong instead of silently relative.
pub fn expand_env(input: &str) -> String {
    expand_with(input, |name| std::env::var(name).ok())
}

fn expand_with(input: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match lookup(name) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "LOCALAPPDATA" => Some("C:\\Users\\me\\AppData\\Local".into()),
            _ => None,
        }
    }

    #[test]
    fn expands_known_variables() {
        assert_eq!(
            expand_with("%LOCALAPPDATA%\\Microsoft\\OneDrive", lookup),
            "C:\\Users\\me\\AppData\\Local\\Microsoft\\OneDrive"
        );
    }

    #[test]
    fn leaves_unknown_variables_visible() {
        assert_eq!(expand_with("%NOPE%\\x", lookup), "%NOPE%\\x");
    }

    #[test]
    fn tolerates_lone_percent() {
        assert_eq!(expand_with("100%", lookup), "100%");
        assert_eq!(expand_with("%%", lookup), "%%");
    }
}
