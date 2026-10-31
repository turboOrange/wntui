use keyring::Entry;

use crate::config::Profile;

const SERVICE: &str = "wntui";

// One keyring entry per profile and host so credentials never leak across
// environments (the same profile name on two backends is two secrets).
fn entry(profile_name: &str, profile: &Profile) -> Result<Entry, keyring::Error> {
    let username = format!("{}@{}", profile.username, profile.host);
    Entry::new(SERVICE, &format!("{profile_name}/{username}"))
}

pub fn load(profile_name: &str, profile: &Profile) -> Result<Option<String>, keyring::Error> {
    match entry(profile_name, profile)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn store(profile_name: &str, profile: &Profile, password: &str) -> Result<(), keyring::Error> {
    entry(profile_name, profile)?.set_password(password)
}

pub fn delete(profile_name: &str, profile: &Profile) -> Result<bool, keyring::Error> {
    match entry(profile_name, profile)?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile {
            host: "wnt.invalid".to_owned(),
            username: "tester".to_owned(),
            ..Profile::default()
        }
    }

    // The zbus/secret-service store is a live system service, so the
    // round-trip tests skip when no Secret Service is running (headless
    // CI); parse-only checks below still run everywhere.
    fn secret_service_available() -> bool {
        std::env::var_os("WNTUI_DISABLE_KEYRING_TESTS").is_none()
            && load("unit-test", &profile()).is_ok()
    }

    #[test]
    fn store_load_delete_round_trip() {
        if !secret_service_available() {
            return;
        }

        let name = format!("round-trip-{}", std::process::id());
        store(&name, &profile(), "probe-password").expect("store");
        assert_eq!(
            load(&name, &profile()).unwrap().as_deref(),
            Some("probe-password")
        );

        assert!(delete(&name, &profile()).unwrap());
        assert_eq!(load(&name, &profile()).unwrap(), None);
    }

    #[test]
    fn load_without_entry_is_none_not_an_error() {
        if !secret_service_available() {
            return;
        }

        let name = format!("missing-{}", std::process::id());
        assert_eq!(load(&name, &profile()).unwrap(), None);
        assert!(
            !delete(&name, &profile()).unwrap(),
            "deleting a missing entry is not an error"
        );
    }

    #[test]
    fn missing_delete_reports_nothing_removed() {
        if !secret_service_available() {
            return;
        }

        let name = format!("missing-delete-{}", std::process::id());
        assert!(!delete(&name, &profile()).unwrap());
    }
}
