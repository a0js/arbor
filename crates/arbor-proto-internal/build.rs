fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc not found");
    unsafe { std::env::set_var("PROTOC", protoc) };

    tonic_build::compile_protos("proto/arbor/v1/arbor.proto")
        .expect("failed to compile proto definitions");
}
