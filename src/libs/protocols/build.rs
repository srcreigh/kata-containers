// Copyright (c) 2020 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::fs;
use std::path::Path;
use std::process::exit;

use ttrpc_codegen::{Codegen, Customize, ProtobufCustomize};

fn replace_text_in_file(file_name: &str, from: &str, to: &str) -> Result<(), std::io::Error> {
    let contents = fs::read_to_string(file_name)?;
    fs::write(file_name, contents.replace(from, to))
}

// The fork's host has an explicit RPC allowlist and decodes only consumed
// response bodies. Keep generated server dispatch (including UNIMPLEMENTED
// methods), but remove unused generated clients and their response decoders.
// Fail closed if the pinned generator changes its output layout.
fn remove_generated_client(path: &str, client: &str, first_handler: &str) -> std::io::Result<()> {
    let mut source = fs::read_to_string(path)?;
    let marker = format!("#[derive(Clone)]\npub struct {client} {{");
    let start = source
        .find(&marker)
        .expect("generated client declaration changed");
    let handler = format!("\nstruct {first_handler} {{");
    let end = start
        + source[start..]
            .find(&handler)
            .expect("generated server boundary changed");
    source.replace_range(start..end, "");
    assert!(
        !source.contains(client),
        "unexpected generated client reference"
    );
    fs::write(path, source)
}

fn codegen(path: &str, protos: &[&str], async_all: bool) -> Result<(), std::io::Error> {
    fs::create_dir_all(path).unwrap();

    // Tell Cargo that if the .proto files changed, to rerun this build script.
    protos
        .iter()
        .for_each(|p| println!("cargo:rerun-if-changed={}", &p));

    let ttrpc_options = Customize {
        async_all,
        ..Default::default()
    };

    let protobuf_options = ProtobufCustomize::default()
        .gen_mod_rs(false)
        .generate_getter(true)
        .generate_accessors(true);

    let out_dir = Path::new("src");

    Codegen::new()
        .out_dir(out_dir)
        .inputs(protos)
        .include("protos")
        .customize(ttrpc_options)
        .rust_protobuf()
        .rust_protobuf_customize(protobuf_options)
        .run()?;

    if protos.contains(&"protos/agent.proto") {
        remove_generated_client(
            "src/agent_ttrpc.rs",
            "AgentServiceClient",
            "CreateContainerMethod",
        )?;
    }
    if protos.contains(&"protos/health.proto") {
        remove_generated_client("src/health_ttrpc.rs", "HealthClient", "CheckMethod")?;
    }

    // ttrpc-codegen 0.6 emits NOT_FOUND for declared methods without an
    // implementation. This fork deliberately excludes features: report
    // UNIMPLEMENTED for those methods, while unknown wire methods remain NOT_FOUND.
    for service in ["agent", "health"] {
        if protos.contains(&format!("protos/{service}.proto").as_str()) {
            replace_text_in_file(
                &format!("src/{service}_ttrpc.rs"),
                "::ttrpc::get_status(::ttrpc::Code::NOT_FOUND,",
                "::ttrpc::get_status(::ttrpc::Code::UNIMPLEMENTED,",
            )?;
        }
    }

    Ok(())
}
fn real_main() -> Result<(), std::io::Error> {
    codegen(
        "src",
        &[
            "protos/google/protobuf/empty.proto",
            "protos/gogo/protobuf/gogoproto/gogo.proto",
            "protos/oci.proto",
            "protos/types.proto",
            "protos/csi.proto",
            "protos/runtimeoptions.proto",
        ],
        false,
    )?;

    // generate async
    #[cfg(feature = "async")]
    {
        codegen("src", &["protos/agent.proto", "protos/health.proto"], true)?;

        fs::rename("src/agent_ttrpc.rs", "src/agent_ttrpc_async.rs")?;
        fs::rename("src/health_ttrpc.rs", "src/health_ttrpc_async.rs")?;
    }

    codegen("src", &["protos/cri-api/api.proto"], false)?;

    // There is a message named 'Box' in oci.proto
    // so there is a struct named 'Box', we should replace Box<Self> to ::std::boxed::Box<Self>
    // to avoid the conflict.
    replace_text_in_file(
        "src/oci.rs",
        "self: Box<Self>",
        "self: ::std::boxed::Box<Self>",
    )?;

    Ok(())
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("ERROR: {e}");
        exit(1);
    }
}
