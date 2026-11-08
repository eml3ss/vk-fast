//! VK authentication identity follows the compiled target, including when the
//! Windows executable is cross-compiled from a Linux build host.

struct ClientIdentity {
    user_agent: &'static str,
    device_name: &'static str,
}

const fn identity_for_windows(windows: bool) -> ClientIdentity {
    if windows {
        ClientIdentity {
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
            device_name: "VK Fast on Windows",
        }
    } else {
        ClientIdentity {
            user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
            device_name: "VK Fast on Linux",
        }
    }
}

pub(crate) const USER_AGENT: &str = identity_for_windows(cfg!(target_os = "windows")).user_agent;
pub(crate) const DEVICE_NAME: &str = identity_for_windows(cfg!(target_os = "windows")).device_name;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_identity_reports_windows_instead_of_the_linux_build_host() {
        let identity = identity_for_windows(true);
        assert!(identity.user_agent.contains("Windows NT 10.0; Win64; x64"));
        assert!(!identity.user_agent.contains("Linux") && !identity.user_agent.contains("X11"));
        assert_eq!(identity.device_name, "VK Fast on Windows");
        assert!(!identity.device_name.contains("Linux"));
    }

    #[test]
    fn linux_identity_keeps_the_existing_browser_version_and_device_name() {
        let identity = identity_for_windows(false);
        assert_eq!(
            identity.user_agent,
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36"
        );
        assert_eq!(identity.device_name, "VK Fast on Linux");
    }

    #[test]
    fn active_authentication_constants_and_music_client_use_the_target_identity() {
        let identity = identity_for_windows(cfg!(target_os = "windows"));
        assert_eq!(USER_AGENT, identity.user_agent);
        assert_eq!(DEVICE_NAME, identity.device_name);
        // The website music client sends this same identity.
        assert_eq!(crate::web_audio::USER_AGENT, USER_AGENT);
    }
}
