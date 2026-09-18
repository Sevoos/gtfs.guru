//! Compiles the vendored GTFS-Realtime schema into Rust types at build time.
//!
//! `protox` is a protobuf compiler written in Rust, so this runs with no
//! `protoc` binary on the machine and CI images need nothing extra installed.
//! It parses the `.proto` into a descriptor set; `prost-build` turns that
//! descriptor into the generated Rust file.
//!
//! Output goes to `OUT_DIR` -- a Cargo-managed directory under `target/` -- and
//! never into `src/`. Generated code is not committed: it is reproduced from the
//! vendored schema on every build, so the schema stays the single source of
//! truth. `src/lib.rs` pulls the result in with `include!`.

use std::env;
use std::path::PathBuf;

use prost::Message;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let proto_dir = manifest_dir.join("proto");
    let proto_file = proto_dir.join("gtfs-realtime.proto");
    let java_proto_dir = proto_dir.join("java-0.0.4");
    let java_proto_file = java_proto_dir.join("gtfs-realtime.proto");

    // Rerun only when an input actually changes; without these, Cargo reruns
    // the script on every build of the crate.
    println!("cargo:rerun-if-changed={}", proto_file.display());
    println!("cargo:rerun-if-changed={}", java_proto_file.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("spec_baseline.json").display()
    );

    let file_descriptors =
        protox::compile([&proto_file], [&proto_dir]).expect("compile vendored gtfs-realtime.proto");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));

    let mut config = prost_build::Config::new();
    config.out_dir(&out_dir);
    // The schema is proto2. Optional scalars therefore arrive as `Option<T>`,
    // which is what the validator needs: GTFS-Realtime v2.0 treats several
    // proto2-optional fields as semantically required, and telling "absent"
    // from "present and zero" is the whole point of those rules.
    config
        .compile_fds(file_descriptors)
        .expect("generate Rust bindings from descriptor set");

    // The current schema generates the public model, while the schema bundled
    // in Java bindings 0.0.4 defines which wire fields and enum values the
    // canonical validator can observe. Keep the latter as a descriptor only:
    // generating a second set of identically named Rust types would make it too
    // easy for rules to use the wrong model.
    let java_descriptors = protox::compile([&java_proto_file], [&java_proto_dir])
        .expect("compile Java 0.0.4 compatibility schema");
    std::fs::write(
        out_dir.join("gtfs-realtime-java-0.0.4.bin"),
        java_descriptors.encode_to_vec(),
    )
    .expect("write Java 0.0.4 compatibility descriptor");
}
