fn main() {
    // Compile proto/wraith.proto into Rust code and write it to OUT_DIR.
    // The generated file is included by src/proto.rs at compile time.
    //
    // The proto lives one directory up from this crate (wraith-rust/../proto/).
    prost_build::compile_protos(
        &["../proto/wraith.proto"],
        &["../proto/"],
    )
    .expect("prost_build failed — check that protoc is installed (apt install protobuf-compiler)");
}
