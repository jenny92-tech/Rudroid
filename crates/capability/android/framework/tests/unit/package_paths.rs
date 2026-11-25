//! INPUT: synthetic package paths
//! OUTPUT: base and split APK resolution
//! POS: framework package path tests
use super::{legacy_base_package_path, split_apk_path};

    #[test]
    fn extracted_only_metadata_keeps_the_legacy_root() {
        let root=std::env::temp_dir().join(format!("rudroid-apk-metadata-extracted-{}",std::process::id()));
        let _=std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dir-assets")).unwrap();
        let extracted=legacy_base_package_path(root.to_str().unwrap());
        assert_eq!(extracted,root.canonicalize().unwrap(),"missing APKs must not poison extracted-only metadata");
        assert!(split_apk_path(root.join("dir-assets").to_str().unwrap()).is_none());
        let _=std::fs::remove_dir_all(root);
    }

    #[test]
    fn real_apk_paths_are_absolute_and_split_directories_are_omitted() {
        let root=std::env::temp_dir().join(format!("rudroid-apk-metadata-real-{}",std::process::id()));
        let _=std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dir-assets")).unwrap();
        std::fs::write(root.join("base.apk"),b"base").unwrap();
        std::fs::write(root.join("config.arm64_v8a.apk"),b"split").unwrap();

        let base=legacy_base_package_path(root.to_str().unwrap());
        assert_eq!(base,root.join("base.apk").canonicalize().unwrap());
        let split=split_apk_path(root.join("config.arm64_v8a.apk").to_str().unwrap()).unwrap();
        assert!(split.is_absolute());
        assert!(split_apk_path(root.join("dir-assets").to_str().unwrap()).is_none());
        assert!(split_apk_path(root.join("missing.apk").to_str().unwrap()).is_none());
        let _=std::fs::remove_dir_all(root);
    }
