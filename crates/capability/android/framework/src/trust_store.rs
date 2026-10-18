//! INPUT:  Linux CA bundle and Unity's Android TrustManagerFactory issuer queries
//! OUTPUT:  real system CA DER bytes through JNI; unsupported Java TLS/custom stores fail explicitly
//! POS:    Android trust-material bridge; certificate and hostname validation stay in native UnityTls
#![forbid(unsafe_code)]

use rudroid_common::jni_abi::{declare_field, declare_superclass};
use rudroid_common::jni_env::{JObject, JniEnv, Native};
use std::ffi::{CStr, CString};
use std::io::Read;
use std::sync::OnceLock;

const FACTORY: &CStr = c"javax/net/ssl/TrustManagerFactory";
const MANAGER: &CStr = c"javax/net/ssl/X509TrustManager";
const CERTIFICATE: &CStr = c"java/security/cert/X509Certificate";
const MAX_BUNDLE: usize = 8 * 1024 * 1024;
const MAX_CERTIFICATES: usize = 4096;
static ROOTS: OnceLock<Result<Vec<Vec<u8>>, String>> = OnceLock::new();

fn throw(env: JniEnv, class: &CStr, message: &str) {
    if let Some(host) = super::HOST.get() {
        host.log_line(c"android_base", &format!("trust store request failed: {message}"));
    }
    let class = env.find_class(class);
    if !class.is_null() {
        let message = CString::new(message.replace('\0', " ")).unwrap();
        env.throw_new(class, &message);
        env.delete_local_ref(class);
    }
}

fn load_roots() -> Result<Vec<Vec<u8>>, String> {
    let configured = std::env::var_os("SSL_CERT_FILE").filter(|path| !path.is_empty());
    let path = if let Some(path) = configured {
        std::path::PathBuf::from(path)
    } else {
        ["/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/cert.pem"]
            .into_iter().map(std::path::PathBuf::from).find(|path| path.is_file())
            .ok_or("No Linux CA bundle found")?
    };
    let mut bytes = Vec::new();
    std::fs::File::open(&path).map_err(|error| format!("CA bundle {}: {error}", path.display()))?
        .take((MAX_BUNDLE + 1) as u64).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    let roots = parse_bundle(&bytes)?;
    if let Some(host) = super::HOST.get() {
        host.log_line(c"android_base", &format!("native TLS trust store: {} system CA certificates from {}", roots.len(), path.display()));
    }
    Ok(roots)
}

fn parse_bundle(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if bytes.len() > MAX_BUNDLE { return Err("CA bundle exceeds 8 MiB".into()); }
    let mut remaining = std::str::from_utf8(bytes).map_err(|_| "CA bundle is not PEM text")?;
    let mut roots = Vec::new();
    while let Some((_, after_begin)) = remaining.split_once("-----BEGIN CERTIFICATE-----") {
        let (body, after_end) = after_begin.split_once("-----END CERTIFICATE-----")
            .ok_or("Unterminated CA certificate")?;
        if roots.len() == MAX_CERTIFICATES { return Err("Too many CA certificates".into()); }
        let der = decode_base64(body.as_bytes()).ok_or("Malformed CA certificate base64")?;
        if !der_sequence(&der) { return Err("Malformed CA certificate DER envelope".into()); }
        roots.push(der);
        remaining = after_end;
    }
    if roots.is_empty() { return Err("CA bundle contains no certificates".into()); }
    Ok(roots)
}

fn decode_base64(bytes: &[u8]) -> Option<Vec<u8>> {
    let compact: Vec<_> = bytes.iter().copied().filter(|byte| !byte.is_ascii_whitespace()).collect();
    if compact.is_empty() || compact.len() % 4 != 0 { return None; }
    let digit = |byte| match byte {
        b'A'..=b'Z' => Some(byte - b'A'), b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52), b'+' => Some(62), b'/' => Some(63), _ => None,
    };
    let mut output = Vec::with_capacity(compact.len() / 4 * 3);
    for (index, part) in compact.chunks_exact(4).enumerate() {
        let a = digit(part[0])?; let b = digit(part[1])?;
        output.push((a << 2) | (b >> 4));
        let last = index + 1 == compact.len() / 4;
        if part[2] == b'=' {
            if !last || part[3] != b'=' || b & 15 != 0 { return None; }
        } else {
            let c = digit(part[2])?;
            output.push((b << 4) | (c >> 2));
            if part[3] == b'=' {
                if !last || c & 3 != 0 { return None; }
            } else { output.push((c << 6) | digit(part[3])?); }
        }
    }
    Some(output)
}

// Validate only the bounded outer DER envelope. UnityTls parses X.509 and
// verifies each connection; this bridge never decides that a peer is trusted.
fn der_sequence(der: &[u8]) -> bool {
    if der.first() != Some(&0x30) { return false; }
    let Some(&length) = der.get(1) else { return false; };
    let (header, length) = if length < 128 { (2, length as usize) } else {
        let count = (length & 127) as usize;
        if count == 0 || count > 4 { return false; }
        let Some(bytes) = der.get(2..2 + count) else { return false; };
        if bytes[0] == 0 { return false; }
        let length = bytes.iter().fold(0usize, |value, &byte| value * 256 + byte as usize);
        if length < 128 { return false; }
        (2 + count, length)
    };
    length > 0 && header + length == der.len()
}

extern "C" fn default_algorithm(env: JniEnv, _class: JObject) -> JObject {
    super::preferences::string(env, "PKIX")
}
extern "C" fn get_instance(env: JniEnv, _class: JObject, algorithm: JObject) -> JObject {
    let Some(algorithm) = env.string_utf16(algorithm).and_then(|value| String::from_utf16(&value).ok()) else {
        throw(env, c"java/lang/NullPointerException", "Trust algorithm is null or invalid");
        return std::ptr::null_mut();
    };
    if !["PKIX", "X509", "SunX509"].iter().any(|name| name.eq_ignore_ascii_case(&algorithm)) {
        throw(env, c"java/security/NoSuchAlgorithmException", "Unsupported trust algorithm");
        return std::ptr::null_mut();
    }
    super::java::allocate(env, FACTORY)
}
extern "C" fn init(env: JniEnv, factory: JObject, store: JObject) {
    if !store.is_null() {
        throw(env, c"java/security/KeyStoreException", "Custom Java KeyStore is unavailable");
        return;
    }
    if let Err(error) = ROOTS.get_or_init(load_roots) {
        throw(env, c"java/security/KeyStoreException", error);
        return;
    }
    let class = env.get_object_class(factory);
    let field = env.get_field_id(class, c"rudroidInitialized", c"Z");
    env.set_boolean_field(factory, field, 1);
    env.delete_local_ref(class);
}
extern "C" fn managers(env: JniEnv, factory: JObject) -> JObject {
    let class = env.get_object_class(factory);
    let field = env.get_field_id(class, c"rudroidInitialized", c"Z");
    let ready = env.get_boolean_field(factory, field) != 0;
    env.delete_local_ref(class);
    if !ready {
        throw(env, c"java/lang/IllegalStateException", "TrustManagerFactory must be initialized");
        return std::ptr::null_mut();
    }
    let class = env.find_class(c"javax/net/ssl/TrustManager");
    let result = env.new_object_array(1, class, std::ptr::null_mut());
    env.delete_local_ref(class);
    if result.is_null() { return result; }
    let manager = super::java::allocate(env, MANAGER);
    if !manager.is_null() { env.set_object_array_element(result, 0, manager); env.delete_local_ref(manager); }
    result
}
extern "C" fn issuers(env: JniEnv, _manager: JObject) -> JObject {
    let Some(Ok(roots)) = ROOTS.get() else {
        throw(env, c"java/lang/IllegalStateException", "System CA store is unavailable");
        return std::ptr::null_mut();
    };
    let class = env.find_class(CERTIFICATE);
    let field = env.get_field_id(class, c"rudroidCertificateId", c"J");
    let result = env.new_object_array(roots.len() as i32, class, std::ptr::null_mut());
    if !result.is_null() {
        for index in 0..roots.len() {
            let certificate = env.alloc_object(class);
            if certificate.is_null() { break; }
            env.set_long_field(certificate, field, index as i64 + 1);
            env.set_object_array_element(result, index as i32, certificate);
            env.delete_local_ref(certificate);
            if env.exception_check() { break; }
        }
    }
    env.delete_local_ref(class);
    result
}
extern "C" fn encoded(env: JniEnv, certificate: JObject) -> JObject {
    let class = env.get_object_class(certificate);
    let field = env.get_field_id(class, c"rudroidCertificateId", c"J");
    let id = env.get_long_field(certificate, field);
    env.delete_local_ref(class);
    let bytes = usize::try_from(id).ok().and_then(|id| id.checked_sub(1))
        .and_then(|index| ROOTS.get().and_then(|roots| roots.as_ref().ok()).and_then(|roots| roots.get(index)));
    let Some(bytes) = bytes else {
        throw(env, c"java/security/cert/CertificateEncodingException", "Unknown system CA certificate");
        return std::ptr::null_mut();
    };
    let result = env.new_byte_array(bytes.len() as i32);
    if !result.is_null() {
        let bytes: Vec<i8> = bytes.iter().map(|&byte| byte as i8).collect();
        env.set_byte_array_region(result, 0, &bytes);
    }
    result
}
extern "C" fn unsupported_check(env: JniEnv, _manager: JObject, _chain: JObject, _auth: JObject) {
    throw(env, c"java/security/cert/CertificateException", "Java TLS verification is unavailable; native UnityTls verifies against the exported system CA roots");
}

pub(super) fn register(env: JniEnv) -> i32 {
    if declare_field(env, FACTORY, c"rudroidInitialized", c"Z", false) != 0
        || declare_field(env, CERTIFICATE, c"rudroidCertificateId", c"J", false) != 0
        || declare_superclass(env, MANAGER, c"javax/net/ssl/TrustManager") != 0
        || declare_superclass(env, CERTIFICATE, c"java/security/cert/Certificate") != 0 { return -1; }
    for (class, natives) in [
        (FACTORY, vec![
            Native::new0(c"getDefaultAlgorithm", c"()Ljava/lang/String;", default_algorithm),
            Native::new1(c"getInstance", c"(Ljava/lang/String;)Ljavax/net/ssl/TrustManagerFactory;", get_instance),
            Native::new1(c"init", c"(Ljava/security/KeyStore;)V", init),
            Native::new0(c"getTrustManagers", c"()[Ljavax/net/ssl/TrustManager;", managers),
        ]),
        (MANAGER, vec![
            Native::new0(c"getAcceptedIssuers", c"()[Ljava/security/cert/X509Certificate;", issuers),
            Native::new2(c"checkServerTrusted", c"([Ljava/security/cert/X509Certificate;Ljava/lang/String;)V", unsupported_check),
            Native::new2(c"checkClientTrusted", c"([Ljava/security/cert/X509Certificate;Ljava/lang/String;)V", unsupported_check),
        ]),
        (CERTIFICATE, vec![Native::new0(c"getEncoded", c"()[B", encoded)]),
        // Unity queries the abstract parent method, then invokes it on an
        // X509Certificate instance. Both lookups use the same actual DER.
        (c"java/security/cert/Certificate", vec![Native::new0(c"getEncoded", c"()[B", encoded)]),
    ] {
        if env.register_class(class, &natives) != 0 { return -1; }
    }
    if env.exception_check() { -1 } else { 0 }
}

#[cfg(test)]
#[path = "../tests/unit/trust_store.rs"]
mod tests;
