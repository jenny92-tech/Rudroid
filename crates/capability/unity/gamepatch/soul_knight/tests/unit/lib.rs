use super::*;

#[test]
fn package_gate_is_exact_and_inert_for_other_titles() {
    assert_eq!(
        identity::selection(Some(PACKAGE), None, 0),
        Selection::Reject("package version is not 8.7.0/80700")
    );
    assert_eq!(identity::selection(Some("com.game.silksong"), None, 0), Selection::Other);
    assert_eq!(identity::selection(None, Some(identity::VERSION_NAME), identity::VERSION_CODE), Selection::Other);
}

#[test]
fn version_mismatch_is_rejected_after_package_selection() {
    assert_eq!(
        identity::selection(Some(PACKAGE), Some("8.6.0"), identity::VERSION_CODE),
        Selection::Reject("package version is not 8.7.0/80700")
    );
    assert_eq!(
        identity::selection(Some(PACKAGE), Some(identity::VERSION_NAME), 80701),
        Selection::Reject("package version is not 8.7.0/80700")
    );
}

#[test]
fn manifest_sdk_declarations_reject_missing_and_legacy_runtime_defaults() {
    assert!(identity::verify_sdk_declarations(23,36).is_ok());
    for (min,target) in [(-1,-1),(23,26),(24,36),(23,35),(0,36),(23,i64::MAX)] {
        assert!(identity::verify_sdk_declarations(min,target).is_err(),"{min}/{target}");
    }
}

#[test]
fn bootstrap_loads_u8t_only_after_sdk_jni_and_initialize() {
    assert_eq!(identity::ORIGINAL_LIBRARIES, [identity::ANOGS, identity::ANORT]);
    assert_eq!(protocol::STATIC_LIBRARY_ORDER, ["anogs", "anort"]);
    assert_eq!(
        identity::BOOTSTRAP_STEPS,
        [
            identity::BootstrapStep::Load(identity::ANOGS),
            identity::BootstrapStep::Load(identity::ANORT),
            identity::BootstrapStep::InitializeAnoApplication,
            identity::BootstrapStep::Load(identity::U8T),
        ]
    );
    assert_eq!(identity::U8T, "lib/arm64-v8a/libu8t.so");
}

#[test]
fn bootstrap_failure_stops_every_later_step() {
    let mut loaded = Vec::new();
    let mut initialized = 0;
    let result = execute_bootstrap(
        |library| {
            loaded.push(library);
            if library == identity::ANORT {
                Err(41)
            } else {
                Ok(())
            }
        },
        || {
            initialized += 1;
            Ok(())
        },
    );
    assert_eq!(
        result,
        Err(BootstrapFailure::Load {
            library: identity::ANORT,
            code: 41,
        })
    );
    assert_eq!(loaded, [identity::ANOGS, identity::ANORT]);
    assert_eq!(initialized, 0, "initialize and u8t must not run after a load failure");
}

#[test]
fn initialize_failure_stops_before_u8t() {
    let mut loaded = Vec::new();
    let result = execute_bootstrap(
        |library| {
            loaded.push(library);
            Ok(())
        },
        || Err(()),
    );
    assert_eq!(result, Err(BootstrapFailure::Initialize));
    assert_eq!(loaded, [identity::ANOGS, identity::ANORT]);
}

#[test]
fn soul_config_has_no_early_u8t_preload() {
    let config = include_str!("../../../../../../../configs/soulknight.toml");
    assert!(config.contains("guest_preload = []"));
    assert!(!config.contains("libu8t.so"));
}

#[test]
fn original_ano_application_declares_seven_natives_without_local_stubs() {
    assert_eq!(protocol::NATIVE_DECLARATIONS.len(), 7);
    assert_eq!(protocol::APPLICATION_CLASS.to_str().unwrap(), "com/ano/gshell/AnoApplication");
    assert_eq!(protocol::APPLICATION_INSTANCE_CLASS.to_str().unwrap(), "com/chillyroomsdk/sdkbridge/BasePlayerApplication");
    assert_eq!(protocol::NATIVE_DECLARATIONS[6].0, protocol::INITIALIZE);
}

#[test]
fn split_contract_is_base_plus_two_real_split_names() {
    assert_eq!(identity::BASE_APK, "base.apk");
    assert_eq!(identity::CONFIG_SPLIT_APK, "config.arm64_v8a.apk");
    assert_eq!(identity::PACK_DATA_SPLIT_APK, "UnityDataAssetPack.apk");
}

#[test]
fn initialize_nonzero_and_exception_are_hard_failures() {
    assert_eq!(initialize_outcome(0, false), Ok(()));
    assert_eq!(
        initialize_outcome(7, false),
        Err(InitializeFailure::ReturnedNonZero)
    );
    assert_eq!(
        initialize_outcome(0, true),
        Err(InitializeFailure::JavaException)
    );
}

#[test]
fn publication_requires_successful_native_attach_and_runs_once() {
    let calls=std::cell::Cell::new(0);
    for (code,exception) in [(7,false),(0,true),(7,true)] {
        assert!(publish_after_initialize(code,exception,||{calls.set(calls.get()+1);true}).is_err());
        assert_eq!(calls.get(),0,"failed native attach must not invoke publisher");
    }
    assert_eq!(publish_after_initialize(0,false,||{calls.set(calls.get()+1);true}),Ok(()));
    assert_eq!(calls.get(),1);
    assert_eq!(publish_after_initialize(0,false,||false),Err(InitializeFailure::PublicationRejected));
}
