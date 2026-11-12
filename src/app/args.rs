/// Value of a command-line flag: None when absent, `Some("")` when present without a value (end of args or the next
/// argument starts with "--").
pub fn arg_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let i = args.iter().position(|a| a == flag)?;
    Some(
        args.get(i + 1)
            .filter(|next| !next.starts_with("--"))
            .cloned()
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_arg_value_absent_is_none() {
        assert!(arg_value("--definitely-absent").is_none());
    }
}
