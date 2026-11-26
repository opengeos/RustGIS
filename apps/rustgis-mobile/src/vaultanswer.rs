//! The answers of Android's secret store (`java/…/SecretVault.java`), read apart from the store
//! itself so the host's tests cover them.

use rustgis_engine::data::secrets::{Secret, SecretError};

/// Reads the Java half's answer: `ok` (with the secret after a tab, for a load that found one),
/// `none`, or `unavailable` or `failed` with the reason after a tab.
pub(crate) fn answer(line: &str) -> Result<Option<Secret>, SecretError> {
    match line.split_once('\t') {
        Some(("ok", secret)) => Ok(Some(Secret::new(secret))),
        Some(("unavailable", why)) => Err(SecretError::Unavailable(why.to_string())),
        Some(("failed", why)) => Err(SecretError::Failed(why.to_string())),
        None if line == "ok" || line == "none" => Ok(None),
        _ => Err(SecretError::Failed("the Android keystore gave an answer RustGIS doesn't know".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_answer_of_the_java_half_is_read() {
        assert_eq!(answer("ok\tsk-test").unwrap().map(|s| s.expose().to_string()), Some("sk-test".into()));
        assert!(answer("ok").unwrap().is_none());
        assert!(answer("none").unwrap().is_none());
        assert_eq!(answer("unavailable\tno keystore"), Err(SecretError::Unavailable("no keystore".into())));
        assert_eq!(answer("failed\tdamaged"), Err(SecretError::Failed("damaged".into())));
        assert!(matches!(answer("saved"), Err(SecretError::Failed(why)) if why.contains("doesn't know")));
        assert!(matches!(answer("later\tx"), Err(SecretError::Failed(why)) if why.contains("doesn't know")));
    }
}
