//! Generates Rust types and service traits from `proto/` and makes sure the
//! embedded UI is re-read when `web/dist` changes.

fn main() {
    let proto_root = "../../proto";
    connectrpc_build::Config::new()
        .files(&[format!("{proto_root}/nethernode/v1/auth.proto")])
        .includes(&[proto_root])
        .include_file("_connectrpc.rs")
        .compile()
        .expect("protobuf code generation failed (is `protoc` on PATH?)");

    println!("cargo:rerun-if-changed={proto_root}");
    println!("cargo:rerun-if-changed=../../web/dist");
}
