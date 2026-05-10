use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set"));
    let schema_path = manifest_dir.join("schema/latlng.capnp");

    println!("cargo:rerun-if-changed={}", schema_path.display());

    let capnp_available = Command::new("capnp").arg("--version").output().is_ok();
    let codegen_result = if capnp_available {
        generate_schema(&schema_path, &out_dir)
    } else {
        Err("capnp compiler is unavailable".to_owned())
    };

    let generated = format!(
        "pub const CAPNP_BINARY_AVAILABLE: bool = {capnp_available};\n\
         pub const CAPNP_CODEGEN_AVAILABLE: bool = {};\n\
         pub const CAPNP_CODEGEN_ERROR: Option<&str> = {};\n\
         pub const SCHEMA_TEXT: &str = include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/schema/latlng.capnp\"));\n",
        codegen_result.is_ok(),
        option_literal(codegen_result.as_ref().err())
    );
    fs::write(out_dir.join("generated.rs"), generated).expect("failed to write schema output");
}

fn generate_schema(schema_path: &Path, out_dir: &Path) -> Result<(), String> {
    capnpc::CompilerCommand::new()
        .src_prefix(
            schema_path
                .parent()
                .expect("schema path should have a parent"),
        )
        .output_path(out_dir)
        .file(schema_path)
        .run()
        .map_err(|error| error.to_string())
}

fn option_literal(error: Option<&String>) -> String {
    match error {
        Some(error) => format!("Some({error:?})"),
        None => "None".to_owned(),
    }
}
