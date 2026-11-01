//! The certificate trust of RustGIS's native HTTPS clients (desktop and command line).
//!
//! Clients verify servers with the operating system's trust (Windows and macOS: their own
//! verifiers, which honour what the system distrusts; Linux and the BSDs: the system bundle, or
//! the `SSL_CERT_FILE` and `SSL_CERT_DIR` the platform verifier reads). The browser uses its own.

use std::sync::LazyLock;

use ureq::tls::{RootCerts, TlsConfig};

/// The TLS settings every native `ureq` agent takes: platform verification, except on a Linux or
/// BSD system with no CA certificates at all (a slim container) and no `SSL_CERT_FILE` or
/// `SSL_CERT_DIR`, where the bundled Mozilla roots keep public HTTPS working. An explicitly
/// chosen trust source is never widened with them.
///
/// The platform verifier reads the system's certificates each time an agent sets up TLS, so an
/// agent should be made once and kept, its requests leaving its TLS settings alone.
pub fn tls_config() -> TlsConfig {
    static ROOTS: LazyLock<RootCerts> = LazyLock::new(|| {
        #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
        if !names_trust_source(std::env::var_os("SSL_CERT_FILE"), std::env::var_os("SSL_CERT_DIR"))
            && rustls_native_certs::load_native_certs().certs.is_empty()
        {
            return RootCerts::WebPki;
        }
        RootCerts::PlatformVerifier
    });
    TlsConfig::builder().root_certs(ROOTS.clone()).build()
}

/// Whether `SSL_CERT_FILE` and `SSL_CERT_DIR` choose the trust source, as `rustls-native-certs`
/// reads them: any `SSL_CERT_FILE` (even empty), or an `SSL_CERT_DIR` naming a directory.
// Only Linux and the BSDs fall back; the rule is tested on every platform.
#[cfg_attr(not(all(unix, not(target_vendor = "apple"), not(target_os = "android"))), allow(dead_code))]
fn names_trust_source(file: Option<std::ffi::OsString>, dirs: Option<std::ffi::OsString>) -> bool {
    file.is_some() || dirs.is_some_and(|d| std::env::split_paths(&d).any(|p| !p.as_os_str().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::names_trust_source;

    #[test]
    fn only_a_named_source_blocks_the_fallback() {
        let s = |v: &str| Some(std::ffi::OsString::from(v));
        assert!(!names_trust_source(None, None));
        // An empty directory list is no list: the system's defaults (or the fallback) apply.
        assert!(!names_trust_source(None, s("")));
        assert!(!names_trust_source(None, s("::")));
        assert!(names_trust_source(None, s(":/etc/ssl/certs")));
        // An empty file is still a file the verifier is told to read.
        assert!(names_trust_source(s(""), None));
    }
}
