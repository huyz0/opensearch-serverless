//! Compiles the vendored OpenSearch protobuf files into a descriptor set the gRPC front loads
//! at run time. No `protoc` is needed: `protox` is a Rust compiler for `.proto` files.

use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let root = PathBuf::from("proto");
    let files = [
        "protos/services/document_service.proto",
        "protos/services/search_service.proto",
    ];
    println!("cargo:rerun-if-changed=proto");
    let set = protox::compile(files, [&root]).expect("the vendored proto files compile");
    let bytes = protox::prost::Message::encode_to_vec(&set);
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("grpc.bin");
    fs::write(out, bytes).expect("the descriptor set is written");
}
