//! Generates Rust types and service traits from `proto/` and makes sure the
//! embedded UI is re-read when `web/dist` changes.

fn main() {
    let proto_root = "../../proto";
    connectrpc_build::Config::new()
        .files(&[
            format!("{proto_root}/nethernode/v1/auth.proto"),
            format!("{proto_root}/nethernode/v1/servers.proto"),
            format!("{proto_root}/nethernode/v1/proxy.proto"),
            format!("{proto_root}/nethernode/v1/files.proto"),
            format!("{proto_root}/nethernode/v1/players.proto"),
            format!("{proto_root}/nethernode/v1/catalog.proto"),
            format!("{proto_root}/nethernode/v1/mods.proto"),
            format!("{proto_root}/nethernode/v1/backups.proto"),
            format!("{proto_root}/nethernode/v1/users.proto"),
            format!("{proto_root}/nethernode/v1/account.proto"),
            format!("{proto_root}/nethernode/v1/audit.proto"),
            format!("{proto_root}/nethernode/v1/notifications.proto"),
            format!("{proto_root}/nethernode/v1/install_count.proto"),
            format!("{proto_root}/nethernode/v1/schedules.proto"),
        ])
        .includes(&[proto_root])
        .include_file("_connectrpc.rs")
        .compile()
        .expect("protobuf code generation failed (is `protoc` on PATH?)");

    println!("cargo:rerun-if-changed={proto_root}");
    println!("cargo:rerun-if-changed=../../web/dist");
}
