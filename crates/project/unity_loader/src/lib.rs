//! INPUT:  rudroid-host-core orchestration surface
//! OUTPUT:  re-exports for the Unity Project CLI and integration tests
//! POS:    thin Project library; host kernel lives in rudroid-host-core
#![forbid(unsafe_code)]

pub use rudroid_host_core::{
    bring_up, graphics_feature_tokens, load_deploy_dir, probe_vulkan, terminate_completed_guest,
    DescriptorInfo, LoadedCapability, LoaderError, GuestPlan, Selection, SelectionError,
    Diagnostic, TEST_BUILD_MARKERS,
};

pub mod guest {
    pub use rudroid_host_core::guest::*;
}

pub mod host_api {
    pub use rudroid_host_core::host_api::*;
}

pub mod host_platform {
    pub use rudroid_host_core::host_platform::*;
}

pub mod phases {
    pub use rudroid_host_core::phases::*;
}

pub mod runtime {
    pub use rudroid_host_core::runtime::*;
}

pub mod select {
    pub use rudroid_host_core::select::*;
}

pub mod validate {
    pub use rudroid_host_core::validate::*;
}
