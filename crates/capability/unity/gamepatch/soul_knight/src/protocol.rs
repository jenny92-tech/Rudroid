//! INPUT:  original 8.7.0 `com.ano.gshell.AnoApplication` declaration surface
//! OUTPUT: class, seven native descriptors and library order owned by this title
//! POS:    protocol source of truth; implementations remain in libanogs/libanort

#![cfg_attr(not(test), forbid(unsafe_code))]
use std::ffi::CStr;

pub const APPLICATION_CLASS: &CStr = c"com/ano/gshell/AnoApplication";
/// The manifest's concrete Application. Its evidenced empty constructor layer
/// adds class identity, not attachBaseContext/onCreate or module initialization.
pub const APPLICATION_INSTANCE_CLASS: &CStr = c"com/chillyroomsdk/sdkbridge/BasePlayerApplication";
pub const INITIALIZE: &CStr = c"initialize";
pub const INITIALIZE_SIGNATURE: &CStr =
    c"(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;)I";

/// The declarations are listed exactly as present in the original class so
/// accidental local stubs or descriptor drift are visible. The shared VM
/// resolves the class and the original `JNI_OnLoad` implementations populate
/// these entries; this adapter registers none of them itself.
#[cfg(test)]
pub const NATIVE_DECLARATIONS: [(&CStr, &CStr); 7] = [
    (c"gp6ioctl", c"(Ljava/lang/String;)V"),
    (c"gp7ioctl", c"(Ljava/lang/String;)V"),
    (c"handleLoad", c"(Ljava/lang/Object;I)Ljava/lang/Object;"),
    (c"handleLoad", c"([BII)Ljava/lang/Object;"),
    (c"handleLoadV22", c"([BII)J"),
    (c"ioctl", c"(ILjava/lang/String;)I"),
    (INITIALIZE, INITIALIZE_SIGNATURE),
];

#[cfg(test)]
pub const STATIC_LIBRARY_ORDER: [&str; 2] = ["anogs", "anort"];
