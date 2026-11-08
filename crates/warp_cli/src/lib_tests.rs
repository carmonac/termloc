use clap::Parser;

use super::*;

#[test]
fn identifies_worker_subcommands() {
    assert!(is_worker_invocation("minidump-server"));
    #[cfg(unix)]
    assert!(is_worker_invocation(&terminal_server_subcommand()));
    assert!(!is_worker_invocation("--prompt"));
}

#[test]
fn global_flags_parse_without_a_subcommand() {
    let args = Args::try_parse_from(["termloc", "--debug"]).unwrap();
    assert!(args.debug());
    assert!(args.command().is_none());
}

#[test]
fn api_key_flag_is_rejected() {
    assert!(Args::try_parse_from(["termloc", "--api-key", "key"]).is_err());
}

#[test]
fn removed_cloud_subcommands_are_rejected() {
    for subcommand in ["agent", "environment", "schedule", "secret", "whoami"] {
        assert!(Args::try_parse_from(["termloc", subcommand]).is_err());
    }
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn acp_bridge_parses_its_launch_file() {
    let args = Args::try_parse_from(["termloc", "acp-bridge", "--launch-file", "/tmp/launch"])
        .expect("acp-bridge should parse");
    assert!(matches!(
        args.command(),
        Some(Command::Worker(WorkerCommand::AcpBridge { .. }))
    ));
}
