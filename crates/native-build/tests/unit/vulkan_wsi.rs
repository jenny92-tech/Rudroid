//! INPUT:  verified fixed WSI cache, existing GNU compiler and a CPU-only C++ fixture
//! OUTPUT:  original-source negative control, patched dispatch/logging and headless-carrier CPU checks
//! POS:    explicit optional-native regression without loading a driver, compositor or game
use super::*;

#[test]
#[ignore = "requires verified WSI native inputs; run with the fixed Docker --vulkan entry"]
fn wsi_contract_rejects_null_dispatch_and_keeps_release_errors() {
    let target = PathBuf::from(std::env::var_os("CARGO_TARGET_DIR").expect("fixed Docker target"));
    let inputs = target.join("native-inputs/vulkan-wsi");
    assert!(inputs.join("receipt.json").is_file(), "verified --vulkan inputs required");
    let output = target.join("native-objects/vulkan-wsi/contracts");
    fs::create_dir_all(&output).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vulkan_wsi_contract.cpp");
    for (directory, patched) in [("wsi", false), ("wsi-patched", true)] {
        let source = inputs.join(directory);
        let binary = output.join(directory);
        let mut compiler = Command::new("g++");
        compiler.args(["-std=c++17", "-O2", "-DNDEBUG", "-pthread", "-Ulinux",
            "-DBUILD_WSI_WAYLAND=1", "-DBUILD_WSI_HEADLESS=0", "-DBUILD_WSI_DISPLAY=0",
            "-DVULKAN_WSI_LAYER_EXPERIMENTAL=0", "-DWSI_IMAGE_COMPRESSION_CONTROL_SWAPCHAIN=0",
            "-DWSI_DISPLAY_SUPPORT_FORMAT_MODIFIERS=0", "-DENABLE_INSTRUMENTATION=0",
            "-DWAYLAND_FIFO_PRESENTATION_THREAD_ENABLED=0"]);
        compiler.arg("-I").arg(&source).arg("-I").arg(inputs.join("headers/include"))
            .arg(&fixture).arg(source.join("util/custom_allocator.cpp"))
            .arg(source.join("util/log.cpp")).arg("-o").arg(&binary);
        if patched {
            compiler.arg("-DRUDROID_WSI_CARRIER_TEST").arg(source.join("util/extension_list.cpp"));
        }
        run(&mut compiler);
        if !patched {
            let result = Command::new(&binary).env_remove("RUDROID_DIAGNOSTICS")
                .env_remove("VULKAN_WSI_DEBUG_LEVEL").output().unwrap();
            assert_eq!(result.status.code(), Some(42), "original engaged-null must fail the fixture");
            continue;
        }
        for (diagnostics, verbosity, evaluations) in [("0", "99", 1), ("1", "3", 3), ("1", "0", 1)] {
            let result = Command::new(&binary).env("RUDROID_DIAGNOSTICS", diagnostics)
                .env("VULKAN_WSI_DEBUG_LEVEL", verbosity).output().unwrap();
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(result.status.success(), "{error}");
            assert_eq!(String::from_utf8_lossy(&result.stdout), format!("evaluations={evaluations} calls=3\ncarrier_cases=16\n"));
            assert!(error.contains("ERROR") && error.contains("fixture-error-1"), "{error}");
            assert_eq!(error.contains("fixture-warning-2"), evaluations == 3, "{error}");
            assert_eq!(error.contains("fixture-info-3"), evaluations == 3, "{error}");
        }
        let normal = Command::new(&binary).env_remove("RUDROID_DIAGNOSTICS")
            .env_remove("VULKAN_WSI_DEBUG_LEVEL").output().unwrap();
        assert!(normal.status.success());
        assert_eq!(String::from_utf8_lossy(&normal.stdout), "evaluations=1 calls=3\ncarrier_cases=16\n");
        let error = String::from_utf8_lossy(&normal.stderr);
        assert!(error.contains("fixture-error-1"));
        assert!(!error.contains("WARNING") && !error.contains("INFO"));
    }
}
