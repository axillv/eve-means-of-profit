use std::fs;
use std::path::{Path, PathBuf};

use mop_jeve::{JeveError, ProfileFile, list_profiles, profiles_dir, profiles_dir_in};

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles_fixture(root: &Path) -> PathBuf {
        let dir = root.join("profiles");
        fs::create_dir(&dir).expect("create profiles folder");
        fs::write(dir.join("#Default.db"), b"").expect("write active profile");
        fs::write(dir.join("Alt.db"), b"").expect("write other profile");
        fs::write(dir.join("#Default.bac"), b"").expect("write backup");
        fs::write(dir.join("#Default_8.2.2_dbbackup.zip"), b"").expect("write zip backup");
        fs::write(dir.join("notes.txt"), b"").expect("write notes");
        fs::create_dir(dir.join("Old.db")).expect("write folder named like a profile");
        dir
    }

    #[test]
    fn lists_only_profile_files_with_the_active_one_first() {
        let root = tempfile::tempdir().expect("temp dir");
        let dir = profiles_fixture(root.path());

        let profiles = list_profiles(&dir).expect("list");

        assert_eq!(
            profiles,
            vec![
                ProfileFile {
                    name: "Default".to_owned(),
                    active: true,
                    path: dir.join("#Default.db"),
                },
                ProfileFile {
                    name: "Alt".to_owned(),
                    active: false,
                    path: dir.join("Alt.db"),
                },
            ]
        );
    }

    #[test]
    fn missing_folder_has_no_profiles() {
        let root = tempfile::tempdir().expect("temp dir");

        let profiles = list_profiles(&root.path().join("not-installed")).expect("list");

        assert!(profiles.is_empty());
    }

    #[test]
    fn a_file_where_the_folder_should_be_is_an_error() {
        let root = tempfile::tempdir().expect("temp dir");
        let file = root.path().join("profiles");
        fs::write(&file, b"").expect("write file");

        let error = list_profiles(&file).expect_err("not a folder");

        assert!(matches!(error, JeveError::Io { path, .. } if path == file));
    }

    #[test]
    fn profile_folder_depends_on_the_platform() {
        assert_eq!(
            profiles_dir_in(Path::new("/home/u"), false),
            PathBuf::from("/home/u/.jeveassets/profiles")
        );
        assert_eq!(
            profiles_dir_in(Path::new("/Users/u"), true),
            PathBuf::from("/Users/u/Library/Preferences/JEveAssets/profiles")
        );
    }

    #[test]
    fn this_system_profile_folder_ends_with_the_platform_folder() {
        let suffix = if cfg!(target_os = "macos") {
            "Library/Preferences/JEveAssets/profiles"
        } else {
            ".jeveassets/profiles"
        };

        let dir = profiles_dir().expect("home folder is known");

        assert!(dir.ends_with(suffix), "{} lacks {suffix}", dir.display());
    }
}
