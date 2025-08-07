use std::io::Result;

fn main() -> Result<()> {
    tonic_prost_build::configure()
        .compile_protos(&["proto/actor_service.proto"], &["proto"])
        .unwrap();
    Ok(())
}
