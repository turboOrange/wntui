use std::io;

fn main() -> io::Result<()> {
    let proto_root = "../../docs/api/protos";

    println!("cargo:rerun-if-changed={proto_root}");

    let file_descriptors = protox::compile([format!("{proto_root}/message.proto")], [proto_root])
        .map_err(io::Error::other)?;

    prost_build::Config::new().compile_fds(file_descriptors)
}
