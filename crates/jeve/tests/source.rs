#[cfg(test)]
mod tests {
    const SOURCES: [(&str, &str); 4] = [
        ("lib.rs", include_str!("../src/lib.rs")),
        ("error.rs", include_str!("../src/error.rs")),
        ("profiles.rs", include_str!("../src/profiles.rs")),
        ("read.rs", include_str!("../src/read.rs")),
    ];

    #[test]
    fn source_never_names_the_secret_columns_or_selects_everything() {
        for (file, source) in SOURCES {
            for forbidden in [
                "refreshtoken",
                "callbackurl",
                "accountname",
                "characterroles",
                "SELECT *",
            ] {
                assert!(!source.contains(forbidden), "{file} contains {forbidden:?}");
            }
        }
    }
}
