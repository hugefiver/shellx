use crate::connection::{ConnectionBackend, ConnectionProfile, DEFAULT_SSH_PORT};
use portable_pty::CommandBuilder;
use wezterm_ssh::{Config, ConfigMap};

pub fn build_system_command(profile: &ConnectionProfile) -> CommandBuilder {
    let mut command = CommandBuilder::new("ssh");
    command.args(system_command_args(profile));

    command
}

fn system_command_args(profile: &ConnectionProfile) -> Vec<String> {
    let mut args = Vec::new();

    if profile.port != DEFAULT_SSH_PORT {
        args.push("-p".to_string());
        args.push(profile.port.to_string());
    }

    if !profile.identity_file.trim().is_empty() {
        args.push("-i".to_string());
        args.push(profile.identity_file.trim().to_string());
    }

    if profile.accept_new_host {
        args.push("-o".to_string());
        args.push("StrictHostKeyChecking=accept-new".to_string());
    }

    args.push(profile.destination());

    if !profile.remote_command.trim().is_empty() {
        args.push(profile.remote_command.trim().to_string());
    }

    args
}

pub fn build_wezterm_config(profile: &ConnectionProfile) -> ConfigMap {
    let mut config = Config::new();
    config.add_default_config_files();

    config.set_option("hostname", profile.host.trim());
    config.set_option("port", profile.port.to_string());

    if !profile.user.trim().is_empty() {
        config.set_option("user", profile.user.trim());
    }

    if !profile.identity_file.trim().is_empty() {
        config.set_option("identityfile", profile.identity_file.trim());
    }

    if profile.accept_new_host {
        config.set_option("stricthostkeychecking", "accept-new");
    }

    let mut resolved = config.for_host(profile.host.trim());
    if !profile.password.trim().is_empty() {
        resolved.insert("password".into(), profile.password.trim().to_string());
    }

    resolved
}

pub fn backend_caption(backend: ConnectionBackend) -> &'static str {
    backend.label()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_command_contains_destination() {
        let mut profile = ConnectionProfile::new("Staging", "staging.internal");
        profile.user = "ops".into();
        profile.port = 2222;

        let args = system_command_args(&profile);
        assert!(args.iter().any(|arg| arg == "ops@staging.internal"));
        assert!(args.iter().any(|arg| arg == "2222"));
        assert_eq!(args.first().map(String::as_str), Some("-p"));
    }

    #[test]
    fn wezterm_config_contains_basic_target_information() {
        let mut profile = ConnectionProfile::new("Prod", "prod.example.com");
        profile.user = "deploy".into();
        let config = build_wezterm_config(&profile);

        assert_eq!(
            config.get("hostname").map(String::as_str),
            Some("prod.example.com")
        );
        assert_eq!(config.get("user").map(String::as_str), Some("deploy"));
        assert_eq!(config.get("port").map(String::as_str), Some("22"));
    }
}
