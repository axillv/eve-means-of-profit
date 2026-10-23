//! The crate names: the packages cargo knows about, and no old names left in the checkout.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    const CRATES: [&str; 6] = ["core", "sde", "market", "store", "esi", "jeve"];

    #[test]
    fn workspace_packages_use_the_mop_names() {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert!(output.status.success());
        let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut names: Vec<&str> = metadata["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|package| package["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "eve-means-of-profit",
                "mop-core",
                "mop-esi",
                "mop-jeve",
                "mop-market",
                "mop-sde",
                "mop-store",
            ]
        );
    }

    /// The paths, relative to `root`, of the files under it that name an old crate. The old names
    /// are built at run time so this file does not match itself.
    fn old_names_in(root: &Path) -> Vec<String> {
        let needles: Vec<String> = CRATES
            .iter()
            .flat_map(|name| [format!("profit-{name}"), format!("profit_{name}")])
            .collect();
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                // file_type does not follow symlinks, so a link out of the checkout is skipped.
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    // Build output, git data and other worktrees are not part of this checkout.
                    if !name.starts_with('.') && name != "target" {
                        pending.push(path);
                    }
                } else if kind.is_file() {
                    // Read as bytes so a file that is not UTF-8 is still searched. A file that
                    // cannot be read fails the test with its name, rather than passing unchecked.
                    let bytes = fs::read(&path)
                        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
                    let text = String::from_utf8_lossy(&bytes);
                    if needles.iter().any(|needle| text.contains(needle)) {
                        found.push(path.strip_prefix(root).unwrap().display().to_string());
                    }
                }
            }
        }
        found
    }

    #[test]
    fn checkout_has_no_old_crate_names() {
        // This crate is at crates/app, so the checkout root is two levels up.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let found = old_names_in(&root);
        assert!(found.is_empty(), "old crate names in: {found:?}");
    }

    #[test]
    fn a_file_that_is_not_utf8_is_still_searched() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = vec![0xff, 0xfe, b' '];
        bytes.extend_from_slice(format!("profit-{}", "core").as_bytes());
        fs::write(dir.path().join("blob.bin"), bytes).unwrap();
        assert_eq!(old_names_in(dir.path()), ["blob.bin"]);
    }
}
